# Echo Research · Rust 单栈计划与验收底账

## 目标

最终仓库只保留 Cargo 工程：Leptos/WASM、axum、Rust application/domain、sqlx/PostgreSQL、Rust worker、定点金融内核和 Rust WebDriver 验收。产品核心是证据优先研究，不提供交易指令。

## 完成度口径（重要）

| 口径 | 含义 | 当前 |
| --- | --- | --- |
| 结构迁移 | Cargo-only；旧 Node/React/Python 运行路径已删除 | **已完成**（PR #43 / `dc4b75c`） |
| 功能平价 | 迁移前保留能力均有 Rust 等价实现、替代说明或退役 ADR | **推进中** — 见下方《当前结构迁移状态》逐区口径 |
| 生产闭环 | HTTPS Web、密钥、备份、Worker 租约、观测、自动集成验收 | **推进中** — 见下方《生产闭环待办》 |

“能编译 / 门禁绿 / 竖切可演示”不等于功能平价完成。任一能力不得标成完成，除非已有验收测试或明确退役 ADR。

## 当前结构迁移状态

| 区域 | Rust 落点 | 结构状态 | 功能平价状态 |
| --- | --- | --- | --- |
| 金融算术 | `finance-core` | 完成 | 完成：Decimal 金额、比率、盈亏、收益惊喜、估值不变量 |
| 意图/估值/护栏 | `echo-domain` | 完成 | 核心可用；需对照恢复后的 QA 语料持续回归 |
| 研究编排 | `echo-application` | 收口中 | `ResearchOrchestrator` 已实现意图相关完备度评估、缺口驱动换源、最多 2 轮与无进展早停；`ResearchService`/报告/流式共用；真实供应商 canary 仍待预生产 |
| HTTP/API | `echo-api` | 基础竖切 | 约 20/45 旧契约；研究链仍偏重 API 边界 |
| 数据库 | `echo-db` | 迁移 + 部分仓储 | auth/workspace/operations/market/scheduler 部分完成；多表缺 Rust 读写 |
| 外部数据 | `echo-data` | 多源竖切 | Finnhub→Yahoo 行情、FMP 主财务、SEC Company Facts 财务补救、HKEX FY PDF 严格解析补救、网页证据/公告/同业/日历已接线；真实供应商 canary 待预生产 |
| 后台 | `echo-worker` | 8 个 cron 定义 | 活动可跑；多实例租约仍需在预生产演练 |
| Web | `echo-web` | 基础页面 | 登录/研究/自选/持仓/设置可用；流式、历史深链、证据卡未平价 |
| 浏览器验收 | `echo-e2e` | 骨架 | 核心流程存在但默认 ignored，需外部 WebDriver |

## 不变量

1. 单公司边界：研究请求中的 ticker 是唯一事实身份。
2. 缺数断口：`None` 代表未核到，永远不回填陈旧/跨公司/零值。
3. Decimal 边界：金融计算不转 `f64`；JSON 和 UI 只在展示处格式化。
4. 租户隔离：RLS + 显式用户条件 + 事务级 `set_config`。
5. 通知策略：偏好、免打扰、去重在 `NotificationsRepository::insert` 唯一咽喉生效。
6. 供应商合规：商用模式排除没有明确商业授权的数据源。
7. 可恢复调度：`scheduler_state` 的 last_run 是唯一恢复游标；活动失败不得记成功。

## 发布门禁

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check -p echo-web --target wasm32-unknown-unknown
cargo xtask web
cargo xtask migrate   # 预生产/生产，显式 DATABASE_URL
cargo xtask e2e       # API、Trunk、WebDriver 已启动时
```

离线研究 QA（意图路由语料）：

```bash
cargo test -p echo-domain --test intent_routing_corpus
```

迁移 `0001`–`0010` 的 SHA-256 冻结于 `docs/qa/fixtures/migration-checksums.json`；新变更只加 `0011+`。

## 生产闭环待办

结构迁移已完成，以下为进入生产前仍需闭合的通道（每条须真数据端到端验证，不以“能编译”代替）：

- **研究主链平价**：统一 `ResearchService` 与完整取数链（FMP 基本面、公告/同业/日历、对比双腿证据）逐条从 pending 转为已验收。
- **Agentic 补救 canary**：用真实缺 TTM EPS 美股、缺 `hk_financials` 港股、估值失败但有同业锚点三组探针，验证 SEC 换源、HKEX FY PDF 回填/仅公告降级、同业对照降级；离线循环与解析测试已覆盖。
- **Worker 生产化**：多实例租约抢占在预生产做双实例竞争演练，确认同一作业同一时刻只有一个实例执行。
- **观测**：`OTEL_EXPORTER_OTLP_ENDPOINT` 非空时挂 OTLP span 导出，API/Worker 每请求/每作业 span，优雅停机排空批处理队列。
- **验收去 ignore**：活库认证、DB 调度状态、DB workspace/RLS、真实浏览器 E2E 四项集成测试当前默认 `#[ignore]`，需外部依赖就绪后转为门禁。
- **部署**：HTTPS Web、密钥注入、镜像 smoke、CI 真浏览器 E2E。

## 后续增强（不改变单栈边界）

- 接入已签署商业协议的行情/财报适配器，并补充真实供应商 canary。
- 将更多一手公告/港交所披露解析接入 `echo-data`，保持 bitemporal 与来源 URL。
- 为研究历史增加报告导出与画像编辑页面；服务端能力需先在矩阵中从 pending 变为已验收。
- 公司档案已自动跨会话读写：只回放遮蔽数字后的定性线索，护栏通过后更新 Markdown 自动段与已核估值字段；后续补“自动段/手工段”在 UI 上的差异化审阅。
- 在预生产跑 RLS 双租户、备份恢复、外部源故障和 Worker 重启联合演练。
