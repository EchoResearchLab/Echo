//! 模型网关——统一的 OpenAI 兼容 HTTP 客户端。
//!
//! OpenAI 兼容协议（DeepSeek 具名预设 → 通用 `MODEL_*` 覆盖层），`POST /chat/completions`。
//! provider 选择在 `echo-config` 完成（唯一环境变量入口）并由调用方显式注入；本模块只负责
//! 请求体构造、作答提取、JSON 解析——都有强类型结构与纯函数单测。流式 SSE 与审计落库分别由
//! API 与数据库层负责。
//!
//! 失败一律返回 `None`，由编排层落到「未核到」。

use std::time::Duration;

pub use echo_config::ModelProviderConfig as ProviderConfig;

/// 调用类别——只影响超时（report 给 120s，其余 60s），与 TS 一致。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnswerKind {
    #[default]
    Chat,
    Router,
    Report,
    Resolver,
}

impl AnswerKind {
    fn as_str(self) -> &'static str {
        match self {
            AnswerKind::Chat => "chat",
            AnswerKind::Router => "router",
            AnswerKind::Report => "report",
            AnswerKind::Resolver => "resolver",
        }
    }

    fn timeout(self) -> Duration {
        match self {
            AnswerKind::Report => Duration::from_secs(120),
            _ => Duration::from_secs(60),
        }
    }
}

/// 作答选项——对齐 TS 的 `ModelAnswerOptions`（流式的 `visibleText` 随 SSE seam 一起接）。
#[derive(Clone, Copy, Debug, Default)]
pub struct ModelAnswerOptions {
    pub kind: AnswerKind,
    /// DeepSeek 专属 `thinking` 开关：`Some` 才附带（`None` 表示不干预 provider 默认）。
    pub thinking: Option<bool>,
    pub max_tokens: Option<u32>,
    pub json: bool,
}

/// 一次成功作答的结果——附回实际 provider/model，便于审计与前端标注。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelAnswer {
    pub content: String,
    pub provider: String,
    pub model: String,
}

/// 构造 `/chat/completions` 请求体（非流式）。纯函数，逐字对齐 TS：`temperature: 0.2`、
/// system+user 两条消息、DeepSeek 才带 `thinking`、可选 `max_tokens`、JSON 模式的
/// `response_format`。抽出来单测请求形状，不必真打网络。
fn build_request_body(
    provider: &ProviderConfig,
    system: &str,
    user: &str,
    options: &ModelAnswerOptions,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": provider.model,
        "temperature": 0.2,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
    });
    let map = body.as_object_mut().expect("json object");

    if provider.id == "deepseek" {
        if let Some(thinking) = options.thinking {
            map.insert(
                "thinking".into(),
                serde_json::json!({ "type": if thinking { "enabled" } else { "disabled" } }),
            );
        }
    }
    if let Some(max_tokens) = options.max_tokens {
        map.insert("max_tokens".into(), serde_json::json!(max_tokens));
    }
    if options.json {
        map.insert(
            "response_format".into(),
            serde_json::json!({ "type": "json_object" }),
        );
    }
    body
}

/// 从非流式响应体里抠出 `choices[0].message.content` 并 trim。空串归一为 `None`。
fn content_from_response(body: &serde_json::Value) -> Option<String> {
    let content = body
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    (!content.is_empty()).then_some(content)
}

/// 从响应体的 `usage` 抠出 (prompt_tokens, completion_tokens)。缺字段即 `None`。纯函数。
fn usage_from_response(body: &serde_json::Value) -> (Option<i32>, Option<i32>) {
    let usage = body.get("usage");
    let get = |key: &str| {
        usage
            .and_then(|u| u.get(key))
            .and_then(serde_json::Value::as_i64)
            .and_then(|n| i32::try_from(n).ok())
    };
    (get("prompt_tokens"), get("completion_tokens"))
}

/// 流式一帧解析结果。OpenAI 兼容 SSE：`data: {...}` 一行一帧，`data: [DONE]` 收尾。
#[derive(Debug, PartialEq, Eq)]
enum SseFrame {
    /// 一段增量正文（`choices[0].delta.content`）。
    Delta(String),
    /// usage 帧（`stream_options.include_usage` 开启时最后附带）。
    Usage(Option<i32>, Option<i32>),
    /// `[DONE]`、非 data 行、空 delta、解析失败——都当无内容跳过。
    Ignore,
}

/// 解析一行 SSE。只认 `data:` 前缀；`[DONE]` 与解析失败归 `Ignore`。纯函数，逐字对齐 TS 的逐行处理。
fn parse_sse_line(line: &str) -> SseFrame {
    let line = line.trim();
    let Some(data) = line.strip_prefix("data:") else {
        return SseFrame::Ignore;
    };
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return SseFrame::Ignore;
    }
    let Ok(json) = serde_json::from_str::<serde_json::Value>(data) else {
        return SseFrame::Ignore;
    };
    // DeepSeek 的每个增量帧都带 `"usage": null`，只有真 usage 对象才算 usage 帧——
    // 拿 `is_some()` 判会把全部 delta 吞成 usage（真 E2E 查出：正文一字不到前端）。
    if json.get("usage").is_some_and(|u| !u.is_null()) {
        let (i, o) = usage_from_response(&json);
        return SseFrame::Usage(i, o);
    }
    let delta = json
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("delta"))
        .and_then(|d| d.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    if delta.is_empty() {
        SseFrame::Ignore
    } else {
        SseFrame::Delta(delta.to_string())
    }
}

/// provider 的增量常常是次字（一页 Markdown 能碎成几百个小 delta）。逐个当独立 SSE 帧下发会让
/// 前端在每个 delta 上重解析整段 Markdown、卡住主线程（真 E2E 回归查出，非臆测）。按 ~24 字符
/// 合并既保住首字延迟收益（填满就吐、不憋到最后），又把事件频率压回合理区间。对齐 TS 的 STREAM_CHUNK_SIZE。
const STREAM_CHUNK_SIZE: usize = 24;

/// 增量合并器：按 `STREAM_CHUNK_SIZE` 攒够才吐，收尾再吐余量。按 char 边界切，不裂多字节。
#[derive(Default)]
struct Coalescer {
    buf: String,
}

impl Coalescer {
    /// 追加一段增量；攒够阈值就返回该吐的整块，否则 `None`。
    fn push(&mut self, delta: &str) -> Option<String> {
        self.buf.push_str(delta);
        (self.buf.chars().count() >= STREAM_CHUNK_SIZE).then(|| std::mem::take(&mut self.buf))
    }

    /// 收尾：把没吐完的余量交出来（空则 `None`）。
    fn flush(&mut self) -> Option<String> {
        (!self.buf.is_empty()).then(|| std::mem::take(&mut self.buf))
    }
}

/// 审计上下文——把审计写入所需的租户池与用户绑一起。给了就每跳都记（成功/失败都记），
/// 写入 best-effort（失败吞掉，绝不阻断模型调用）。不给（`None`）就纯作答不落审计。
#[derive(Clone, Copy)]
pub struct AuditContext<'a> {
    pub pool: &'a echo_db::Pool,
    pub user_id: &'a str,
}

impl AuditContext<'_> {
    /// best-effort 写一条审计——任何失败（连不上库、RLS 拒绝、写冲突）都吞掉，不让审计阻断作答。
    /// `user_id` 由本上下文注入，其余业务字段由调用方在 `entry` 里给全。
    async fn record(&self, mut entry: echo_db::LlmAuditEntry) {
        entry.user_id = self.user_id.to_string();
        let _ = echo_db::LlmAuditRepository::new(self.pool)
            .insert(&entry)
            .await;
    }
}

/// 统一作答入口（非流式）。`provider` 由调用方在进程边界经 `echo-config` 解析后注入——本函数
/// 不再读 env。没配任何 provider 时（`None`）直接归一到 `None`，对齐 TS「失败返回 null」。
/// 打了 OpenAI 兼容端点后任何失败（网络错、非 2xx、空正文）也归一到 `None`。给了 `audit` 就每跳
/// 落一条审计（成功记 tokens/时延，失败记 error_detail），审计写入 best-effort，绝不阻断作答。
pub async fn model_answer(
    system: &str,
    user: &str,
    options: ModelAnswerOptions,
    provider: Option<&ProviderConfig>,
    audit: Option<AuditContext<'_>>,
) -> Option<ModelAnswer> {
    let provider = provider?;
    let url = format!("{}/chat/completions", provider.base.trim_end_matches('/'));
    let body = build_request_body(provider, system, user, &options);
    let kind = options.kind.as_str();
    let started = std::time::Instant::now();

    // 组一条审计条目：provider/model/kind/时延固定，只有 status/error/tokens 随成功或失败变。
    let make_entry = |status: &str,
                      error_detail: Option<String>,
                      input_tokens: Option<i32>,
                      output_tokens: Option<i32>| {
        echo_db::LlmAuditEntry {
            user_id: String::new(), // AuditContext::record 注入
            provider: provider.id.clone(),
            model: Some(provider.model.clone()),
            kind: kind.to_string(),
            status: status.to_string(),
            latency_ms: Some(started.elapsed().as_millis().try_into().unwrap_or(i32::MAX)),
            error_detail,
            input_tokens,
            output_tokens,
            estimated_cost_usd: None,
        }
    };

    // 失败即记一条 error 审计后返回 None——把「记审计 + 归一 None」收成一处，避免每条错路重复。
    macro_rules! fail {
        ($detail:expr) => {{
            if let Some(ctx) = &audit {
                ctx.record(make_entry("error", Some($detail), None, None))
                    .await;
            }
            return None;
        }};
    }

    let client = reqwest::Client::new();
    let response = match client
        .post(&url)
        .bearer_auth(&provider.key)
        .json(&body)
        .timeout(options.kind.timeout())
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => fail!(e.to_string()),
    };
    if !response.status().is_success() {
        fail!(format!("model {}", response.status().as_u16()));
    }
    let payload: serde_json::Value = match response.json().await {
        Ok(p) => p,
        Err(e) => fail!(e.to_string()),
    };
    let Some(content) = content_from_response(&payload) else {
        fail!("empty model content".to_string());
    };
    let (input_tokens, output_tokens) = usage_from_response(&payload);
    if let Some(ctx) = &audit {
        ctx.record(make_entry("ok", None, input_tokens, output_tokens))
            .await;
    }
    Some(ModelAnswer {
        content,
        provider: provider.id.clone(),
        model: provider.model.clone(),
    })
}

/// 拥有所有权的审计上下文——流式驱动在 `tokio::spawn` 的 `'static` 任务里落审计，不能借用池，
/// 故持有 `Pool`（Arc 支撑，克隆廉价）与 `user_id` 的所有权。
#[derive(Clone)]
pub struct OwnedAuditContext {
    pub pool: echo_db::Pool,
    pub user_id: String,
}

impl OwnedAuditContext {
    async fn record(&self, mut entry: echo_db::LlmAuditEntry) {
        entry.user_id = self.user_id.clone();
        let _ = echo_db::LlmAuditRepository::new(&self.pool)
            .insert(&entry)
            .await;
    }
}

/// 模型流式事件——区分正文块、干净结束与失败，避免通道关闭被误当成成功。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelStreamEvent {
    Delta(String),
    Completed,
    Failed { message: String },
}

/// 流式作答启动结果。
#[derive(Debug)]
pub enum ModelStreamStart {
    /// 未配置任何 provider。
    Unavailable,
    /// 已启动后台拉流任务。
    Ready(tokio::sync::mpsc::Receiver<ModelStreamEvent>),
}

/// 流式作答——返回终端可辨的事件流（对齐 TS 的 `readStreamedCompletion`）。
/// 内部 spawn 读 OpenAI 兼容 `stream:true` SSE，按 [`Coalescer`] 合并成 ~24 字符块。
/// 干净结束发 [`ModelStreamEvent::Completed`]；无 provider 返回 [`ModelStreamStart::Unavailable`]；
/// 网络/HTTP/读失败发 [`ModelStreamEvent::Failed`]。
///
/// 显式 seam：TS 的 `visibleText`（剥 FALSIFIERS_JSON 机器行）属研究编排层裁剪，随该层接上；
/// 此处只做协议级合并，不掺业务裁剪。
#[must_use]
pub fn model_answer_stream(
    system: String,
    user: String,
    options: ModelAnswerOptions,
    provider: Option<ProviderConfig>,
    audit: Option<OwnedAuditContext>,
) -> ModelStreamStart {
    use futures_util::StreamExt;

    let Some(provider) = provider else {
        return ModelStreamStart::Unavailable;
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<ModelStreamEvent>(32);
    tokio::spawn(async move {
        let url = format!("{}/chat/completions", provider.base.trim_end_matches('/'));
        let mut body = build_request_body(&provider, &system, &user, &options);
        // 开流式 + 要 usage 帧（对齐 TS：stream:true / stream_options.include_usage）。
        if let Some(map) = body.as_object_mut() {
            map.insert("stream".into(), serde_json::json!(true));
            map.insert(
                "stream_options".into(),
                serde_json::json!({ "include_usage": true }),
            );
        }
        let started = std::time::Instant::now();
        let kind = options.kind.as_str();
        let make_entry = |status: &str,
                          error: Option<String>,
                          input_tokens: Option<i32>,
                          output_tokens: Option<i32>| {
            echo_db::LlmAuditEntry {
                user_id: String::new(),
                provider: provider.id.clone(),
                model: Some(provider.model.clone()),
                kind: kind.to_string(),
                status: status.to_string(),
                latency_ms: Some(started.elapsed().as_millis().try_into().unwrap_or(i32::MAX)),
                error_detail: error,
                input_tokens,
                output_tokens,
                estimated_cost_usd: None,
            }
        };

        let fail = |message: String| async {
            if let Some(ctx) = &audit {
                ctx.record(make_entry("error", Some(message.clone()), None, None))
                    .await;
            }
            let _ = tx.send(ModelStreamEvent::Failed { message }).await;
        };

        let client = reqwest::Client::new();
        let response = match client
            .post(&url)
            .bearer_auth(&provider.key)
            .json(&body)
            .timeout(options.kind.timeout())
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                fail(format!("model {}", r.status().as_u16())).await;
                return;
            }
            Err(e) => {
                fail(e.to_string()).await;
                return;
            }
        };

        // 逐字节读，按 '\n' 切行喂 parse_sse_line；跨块残行留在 buf。
        let mut stream = response.bytes_stream();
        let mut line_buf = String::new();
        let mut coalescer = Coalescer::default();
        let mut input_tokens = None;
        let mut output_tokens = None;
        let mut errored = None;
        while let Some(chunk) = stream.next().await {
            let bytes = match chunk {
                Ok(b) => b,
                Err(e) => {
                    errored = Some(e.to_string());
                    break;
                }
            };
            line_buf.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(nl) = line_buf.find('\n') {
                let line: String = line_buf.drain(..=nl).collect();
                match parse_sse_line(&line) {
                    SseFrame::Delta(delta) => {
                        if let Some(block) = coalescer.push(&delta) {
                            if tx.send(ModelStreamEvent::Delta(block)).await.is_err() {
                                // 接收端已丢弃（客户端取消/断开）——停读前落一条 cancelled 审计，
                                // 否则这次真实发生过的模型调用在审计表里完全不可见。
                                if let Some(ctx) = &audit {
                                    ctx.record(make_entry(
                                        "cancelled",
                                        None,
                                        input_tokens,
                                        output_tokens,
                                    ))
                                    .await;
                                }
                                return;
                            }
                        }
                    }
                    SseFrame::Usage(i, o) => {
                        input_tokens = i;
                        output_tokens = o;
                    }
                    SseFrame::Ignore => {}
                }
            }
        }
        if let Some(block) = coalescer.flush() {
            let _ = tx.send(ModelStreamEvent::Delta(block)).await;
        }
        if let Some(ctx) = &audit {
            let entry = match &errored {
                Some(detail) => {
                    make_entry("error", Some(detail.clone()), input_tokens, output_tokens)
                }
                None => make_entry("ok", None, input_tokens, output_tokens),
            };
            ctx.record(entry).await;
        }
        if let Some(detail) = errored {
            let _ = tx.send(ModelStreamEvent::Failed { message: detail }).await;
        } else {
            let _ = tx.send(ModelStreamEvent::Completed).await;
        }
    });
    ModelStreamStart::Ready(rx)
}

/// 去掉 ```json 围栏后解析成 JSON 对象；任何解析失败返回 `None`。对齐 TS 的 `parseJsonObject`。
#[must_use]
pub fn parse_json_object(content: &str) -> Option<serde_json::Value> {
    let text = content.trim();
    // 去掉起始的 ```json / ``` 围栏与结尾的 ```。
    let text = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```JSON"))
        .or_else(|| text.strip_prefix("```"))
        .unwrap_or(text)
        .trim_start();
    let text = text.strip_suffix("```").unwrap_or(text).trim();
    serde_json::from_str(text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deepseek_body_carries_thinking_and_options() {
        let provider = ProviderConfig {
            id: "deepseek".into(),
            key: "k".into(),
            base: "https://api.deepseek.com".into(),
            model: "deepseek-v4-flash".into(),
        };
        let body = build_request_body(
            &provider,
            "sys",
            "usr",
            &ModelAnswerOptions {
                thinking: Some(true),
                max_tokens: Some(1024),
                json: true,
                ..Default::default()
            },
        );
        assert_eq!(body["temperature"], serde_json::json!(0.2));
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "usr");
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["max_tokens"], serde_json::json!(1024));
        assert_eq!(body["response_format"]["type"], "json_object");
    }

    #[test]
    fn non_deepseek_body_omits_thinking() {
        let provider = ProviderConfig {
            id: "generic".into(),
            key: "k".into(),
            base: "https://llm.internal/v1".into(),
            model: "gpt-5-mini".into(),
        };
        let body = build_request_body(
            &provider,
            "sys",
            "usr",
            &ModelAnswerOptions {
                thinking: Some(true),
                ..Default::default()
            },
        );
        assert!(body.get("thinking").is_none());
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn content_extracted_and_trimmed() {
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "  hello  " } }]
        });
        assert_eq!(content_from_response(&body).as_deref(), Some("hello"));
    }

    #[test]
    fn empty_content_is_none() {
        let body = serde_json::json!({ "choices": [{ "message": { "content": "   " } }] });
        assert!(content_from_response(&body).is_none());
        assert!(content_from_response(&serde_json::json!({})).is_none());
    }

    #[test]
    fn parse_json_object_strips_fence() {
        let parsed = parse_json_object("```json\n{\"a\": 1}\n```").expect("json");
        assert_eq!(parsed["a"], serde_json::json!(1));
        let bare = parse_json_object("{\"b\": 2}").expect("json");
        assert_eq!(bare["b"], serde_json::json!(2));
    }

    #[test]
    fn parse_json_object_bad_input_is_none() {
        assert!(parse_json_object("not json").is_none());
        assert!(parse_json_object("").is_none());
    }

    #[test]
    fn usage_extracted_when_present() {
        let body = serde_json::json!({
            "usage": { "prompt_tokens": 120, "completion_tokens": 45 }
        });
        assert_eq!(usage_from_response(&body), (Some(120), Some(45)));
    }

    #[test]
    fn usage_missing_is_none_pair() {
        assert_eq!(usage_from_response(&serde_json::json!({})), (None, None));
    }

    #[test]
    fn sse_line_parses_content_delta() {
        let line = r#"data: {"choices":[{"delta":{"content":"你好"}}]}"#;
        assert_eq!(parse_sse_line(line), SseFrame::Delta("你好".to_string()));
    }

    #[test]
    fn sse_done_and_noise_are_ignored() {
        assert_eq!(parse_sse_line("data: [DONE]"), SseFrame::Ignore);
        assert_eq!(parse_sse_line(": comment"), SseFrame::Ignore);
        assert_eq!(parse_sse_line("data: not-json"), SseFrame::Ignore);
        assert_eq!(
            parse_sse_line(r#"data: {"choices":[{"delta":{}}]}"#),
            SseFrame::Ignore
        );
    }

    #[test]
    fn sse_usage_frame_extracted() {
        let line = r#"data: {"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":20}}"#;
        assert_eq!(parse_sse_line(line), SseFrame::Usage(Some(10), Some(20)));
    }

    #[test]
    fn sse_delta_with_null_usage_is_still_delta() {
        // DeepSeek 真实帧形状：开 include_usage 后每个增量帧都带 "usage": null，
        // 内容帧还并排一个 "reasoning_content": null——都不能把 delta 吞成 usage 帧。
        let line = r#"data: {"choices":[{"index":0,"delta":{"content":"苹果","reasoning_content":null},"finish_reason":null}],"usage":null}"#;
        assert_eq!(parse_sse_line(line), SseFrame::Delta("苹果".to_string()));
    }

    #[test]
    fn coalescer_holds_until_threshold_then_flushes_remainder() {
        let mut c = Coalescer::default();
        assert_eq!(c.push("short"), None); // < 24 字符，先攒
        // 攒过 24 字符触发一块。
        let block = c.push(&"x".repeat(30)).expect("block");
        assert!(block.chars().count() >= STREAM_CHUNK_SIZE);
        assert_eq!(c.flush(), None); // 已清空
        c.push("尾巴");
        assert_eq!(c.flush().as_deref(), Some("尾巴")); // 余量收尾吐出
    }

    #[test]
    fn coalescer_respects_char_boundary() {
        let mut c = Coalescer::default();
        // 24 个中文（72 字节）达 24 字符阈值，整块吐出且按 char 对齐，不裂多字节。
        let block = c.push(&"错".repeat(24)).expect("block");
        assert_eq!(block.chars().count(), 24);
        assert!(block.is_char_boundary(block.len()));
    }
}
