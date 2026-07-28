use serde::Serialize;
use serde::de::DeserializeOwned;

// 会话失效的回调，由 `App` 在根注入。
//
// 服务端对过期会话返回 401「请先登录」。此前这条和别的错误一样被当成字符串内联渲染，
// 于是用户停在一个每块面板都写着"请先登录"的工作台上——按钮还在、点了还是 401，
// 没有任何东西告诉他"你该重新登录了"，只能自己想到去刷新页面。会话过期是产品必然
// 会发生的状态，不是异常，必须自己走回登录页。
//
// WASM 是单线程，用 `thread_local` 存回调即可，不必为此引入自定义 DOM 事件。
// SESSION_LOST：已经通知过就不再重复——一屏上并发的多个请求会同时撞 401，
// 每个都触发一次重新鉴权是纯浪费。任何一次成功响应都会把它复位。
#[cfg(target_arch = "wasm32")]
thread_local! {
    static ON_UNAUTHORIZED: std::cell::RefCell<Option<leptos::Callback<()>>> =
        const { std::cell::RefCell::new(None) };
    static SESSION_LOST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// 注册会话失效回调。只在根组件调用一次。
#[cfg(target_arch = "wasm32")]
pub fn on_unauthorized(callback: leptos::Callback<()>) {
    ON_UNAUTHORIZED.with(|slot| *slot.borrow_mut() = Some(callback));
}

#[cfg(not(target_arch = "wasm32"))]
pub fn on_unauthorized(_callback: leptos::Callback<()>) {}

#[cfg(target_arch = "wasm32")]
fn notify_unauthorized() {
    if SESSION_LOST.with(std::cell::Cell::get) {
        return;
    }
    SESSION_LOST.with(|flag| flag.set(true));
    use leptos::Callable;
    let callback = ON_UNAUTHORIZED.with(|slot| *slot.borrow());
    if let Some(callback) = callback {
        callback.call(());
    }
}

#[cfg(target_arch = "wasm32")]
async fn decode<T: DeserializeOwned>(
    path: &str,
    response: gloo_net::http::Response,
) -> Result<T, String> {
    let status = response.status();
    if response.ok() {
        SESSION_LOST.with(|flag| flag.set(false));
        return response
            .json::<T>()
            .await
            .map_err(|error| format!("响应解析失败：{error}"));
    }
    // `/api/auth/*` 自己就是登录流程：登录密码错也是 401，拿它去"回登录页"既没意义，
    // 还会把刚渲染出来的错误提示冲掉。
    if status == 401 && !path.starts_with("/api/auth/") {
        notify_unauthorized();
    }
    let body = response.text().await.unwrap_or_default();
    let message = serde_json::from_str::<echo_contracts::ErrorResponse>(&body)
        .map(|error| error.message)
        .unwrap_or_else(|_| format!("服务返回 {status}"));
    Err(message)
}

#[cfg(target_arch = "wasm32")]
pub async fn get<T: DeserializeOwned>(path: &str) -> Result<T, String> {
    let response = gloo_net::http::Request::get(path)
        .send()
        .await
        .map_err(|error| format!("请求失败：{error}"))?;
    decode(path, response).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn get<T: DeserializeOwned>(_path: &str) -> Result<T, String> {
    Err("网络请求只在 WASM 浏览器目标执行".into())
}

#[cfg(target_arch = "wasm32")]
pub async fn post<I: Serialize + ?Sized, O: DeserializeOwned>(
    path: &str,
    input: &I,
) -> Result<O, String> {
    let response = gloo_net::http::Request::post(path)
        .json(input)
        .map_err(|error| format!("请求编码失败：{error}"))?
        .send()
        .await
        .map_err(|error| format!("请求失败：{error}"))?;
    decode(path, response).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn post<I: Serialize + ?Sized, O: DeserializeOwned>(
    _path: &str,
    _input: &I,
) -> Result<O, String> {
    Err("网络请求只在 WASM 浏览器目标执行".into())
}

#[cfg(target_arch = "wasm32")]
pub async fn patch<I: Serialize + ?Sized, O: DeserializeOwned>(
    path: &str,
    input: &I,
) -> Result<O, String> {
    let response = gloo_net::http::Request::patch(path)
        .json(input)
        .map_err(|error| format!("请求编码失败：{error}"))?
        .send()
        .await
        .map_err(|error| format!("请求失败：{error}"))?;
    decode(path, response).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn patch<I: Serialize + ?Sized, O: DeserializeOwned>(
    _path: &str,
    _input: &I,
) -> Result<O, String> {
    Err("网络请求只在 WASM 浏览器目标执行".into())
}

#[cfg(target_arch = "wasm32")]
pub async fn put<I: Serialize + ?Sized, O: DeserializeOwned>(
    path: &str,
    input: &I,
) -> Result<O, String> {
    let response = gloo_net::http::Request::put(path)
        .json(input)
        .map_err(|error| format!("请求编码失败：{error}"))?
        .send()
        .await
        .map_err(|error| format!("请求失败：{error}"))?;
    decode(path, response).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn put<I: Serialize + ?Sized, O: DeserializeOwned>(
    _path: &str,
    _input: &I,
) -> Result<O, String> {
    Err("网络请求只在 WASM 浏览器目标执行".into())
}

#[cfg(target_arch = "wasm32")]
pub async fn delete<O: DeserializeOwned>(path: &str) -> Result<O, String> {
    let response = gloo_net::http::Request::delete(path)
        .send()
        .await
        .map_err(|error| format!("请求失败：{error}"))?;
    decode(path, response).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn delete<O: DeserializeOwned>(_path: &str) -> Result<O, String> {
    Err("网络请求只在 WASM 浏览器目标执行".into())
}

/// 一次类型化 SSE 流的取消句柄——持有 `AbortController` 让上层随时中止底层 fetch。
#[cfg(target_arch = "wasm32")]
#[derive(Clone)]
pub struct StreamHandle {
    controller: std::rc::Rc<web_sys::AbortController>,
}

#[cfg(target_arch = "wasm32")]
impl StreamHandle {
    pub fn cancel(&self) {
        self.controller.abort();
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub struct StreamHandle;

#[cfg(not(target_arch = "wasm32"))]
impl StreamHandle {
    pub fn cancel(&self) {}
}

/// 发起一次类型化研究 SSE 流：逐帧解析 `data:` 行为 `echo_contracts::ResearchStreamEvent`
/// 并回调 `on_event`；连接失败或流异常终止时回调 `on_error` 一次。返回的 [`StreamHandle`]
/// 可用于用户主动取消（底层走 `AbortController`，服务端因 SSE 接收端断开而停止生成）。
#[cfg(target_arch = "wasm32")]
pub fn post_stream<I: Serialize + ?Sized>(
    path: &str,
    input: &I,
    mut on_event: impl FnMut(echo_contracts::ResearchStreamEvent) + 'static,
    on_error: impl FnOnce(String) + 'static,
) -> StreamHandle {
    use wasm_bindgen::JsCast;

    let controller = web_sys::AbortController::new().expect("AbortController::new 不应失败");
    let signal = controller.signal();
    let request = gloo_net::http::Request::post(path)
        .abort_signal(Some(&signal))
        .json(input);

    wasm_bindgen_futures::spawn_local(async move {
        let outcome: Result<(), String> = async {
            let request = request.map_err(|error| format!("请求编码失败：{error}"))?;
            let response = request
                .send()
                .await
                .map_err(|error| format!("请求失败：{error}"))?;
            if !response.ok() {
                // 研究流是最长的一条请求，也最容易横跨会话过期的那一刻。
                if response.status() == 401 {
                    notify_unauthorized();
                }
                return Err(format!("服务返回 {}", response.status()));
            }
            let stream = response.body().ok_or_else(|| "响应无正文流".to_string())?;
            let reader = stream
                .get_reader()
                .unchecked_into::<web_sys::ReadableStreamDefaultReader>();
            let mut buf: Vec<u8> = Vec::new();
            loop {
                let chunk = wasm_bindgen_futures::JsFuture::from(reader.read())
                    .await
                    .map_err(|error| format!("{error:?}"))?;
                let done = js_sys::Reflect::get(&chunk, &"done".into())
                    .ok()
                    .and_then(|value| value.as_bool())
                    .unwrap_or(true);
                if done {
                    break;
                }
                let Some(value) = js_sys::Reflect::get(&chunk, &"value".into()).ok() else {
                    continue;
                };
                let Ok(bytes) = value.dyn_into::<js_sys::Uint8Array>() else {
                    continue;
                };
                buf.extend(bytes.to_vec());
                while let Some(end) = find_frame_end(&buf) {
                    let frame: Vec<u8> = buf.drain(..end).collect();
                    if let Some(event) = parse_sse_frame(&frame) {
                        on_event(event);
                    }
                }
            }
            Ok(())
        }
        .await;
        if let Err(message) = outcome {
            on_error(message);
        }
    });

    StreamHandle {
        controller: std::rc::Rc::new(controller),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn post_stream<I: Serialize + ?Sized>(
    _path: &str,
    _input: &I,
    _on_event: impl FnMut(echo_contracts::ResearchStreamEvent) + 'static,
    on_error: impl FnOnce(String) + 'static,
) -> StreamHandle {
    on_error("网络请求只在 WASM 浏览器目标执行".into());
    StreamHandle
}

/// 触发浏览器把一段文本以文件形式下载——纯客户端 Blob + `<a download>`，不经服务端往返，
/// 不引入任何 JS 依赖。
#[cfg(target_arch = "wasm32")]
pub fn download_text_file(filename: &str, mime: &str, content: &str) {
    use wasm_bindgen::JsCast;

    let parts = js_sys::Array::new();
    parts.push(&wasm_bindgen::JsValue::from_str(content));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(mime);
    let Ok(blob) = web_sys::Blob::new_with_str_sequence_and_options(&parts, &options) else {
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };
    if let Some(document) = leptos::window().document() {
        if let Ok(element) = document.create_element("a") {
            if let Ok(anchor) = element.dyn_into::<web_sys::HtmlAnchorElement>() {
                anchor.set_href(&url);
                anchor.set_download(filename);
                anchor.click();
            }
        }
    }
    let _ = web_sys::Url::revoke_object_url(&url);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn download_text_file(_filename: &str, _mime: &str, _content: &str) {}

/// 把一段文本写进系统剪贴板——纯浏览器 API，不经服务端。写入失败（无权限/非安全上下文）
/// 时静默返回：复制不是关键路径，不该为它弹错误。
#[cfg(target_arch = "wasm32")]
pub fn copy_text(text: &str) {
    let clipboard = leptos::window().navigator().clipboard();
    let _ = clipboard.write_text(text);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn copy_text(_text: &str) {}

/// 找到下一帧结尾（`\n\n`），返回其后一个字节的偏移，供 `drain` 一次取走整帧（含分隔符）。
#[cfg(target_arch = "wasm32")]
fn find_frame_end(buf: &[u8]) -> Option<usize> {
    buf.windows(2)
        .position(|window| window == b"\n\n")
        .map(|pos| pos + 2)
}

/// 从一帧原始 SSE 文本里取出 `data:` 行并反序列化。忽略 keep-alive 注释帧（无 `data:` 行）。
#[cfg(target_arch = "wasm32")]
fn parse_sse_frame(frame: &[u8]) -> Option<echo_contracts::ResearchStreamEvent> {
    let text = String::from_utf8_lossy(frame);
    let data_line = text.lines().find_map(|line| line.strip_prefix("data:"))?;
    serde_json::from_str(data_line.trim()).ok()
}
