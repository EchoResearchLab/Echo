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
cargo xtask frozen    # 冻结检查（已并入 cargo xtask check）
cargo xtask web
cargo xtask migrate   # 预生产/生产，显式 DATABASE_URL
cargo xtask e2e       # API、Trunk、WebDriver 已启动时
```

### 冻结门禁

`cargo xtask frozen` 拦本仓库的头号缺陷——**写好了没人调**。它检查两件事，编译器永远不会替我们检查：

1. `migrations/` 里建了且未 drop 的活表，必须在某个 crate 的 Rust 源码里出现。
2. `echo-api` 注册的 `/api/...` 路由，必须有 `echo-web`（或 `echo-e2e`）侧的调用方。

两者都可以豁免，但豁免必须显式登记在 `docs/qa/frozen-registry.json` 并写清理由与去向。登记表是双向的：**登记了却其实已经接线的条目同样判失败**，所以这张表只会缩短，不会变成永久赦免书。`_archive_*` 存档表按定义跳过。

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
- **冻结表清账**：`docs/qa/frozen-registry.json` 里登记的 10 张活表各自要么接回、要么出退役 ADR 并 drop；每接掉一张就从登记表删一行。当前最该先清的是 `research_facts` / `research_questions`（与 `company_profiles` 自动段职责重叠，属历史遗留）。

## 2026-07-28 冻结审查已闭合

- **组合快照不再伪造盈亏**：`capture_portfolio_snapshot` 曾用 `coalesce(avg_cost, 0)`，把"用户未填成本"当成本 0，整笔市值被记成盈利并写进 `portfolio_snapshots`。现在缺成本即断口（`missing_cost`），成本与盈亏一起抹成 `None`，缺任一项都不落快照；worker 把"未填成本价跳过"与"缺价/汇率跳过"分开报。
- **护栏结论落库**：`fact_guard_audit` 建表以来无人写。现在每条作答链路（`ask` / `ask_stream` / `compare` / `report`）都经 `ResearchPorts::record_guard_audit` 记一条（计数 + 硬失败明细），与返回给前端的 `GuardView` 同源同一次 `verify_answer_numbers`。无库或写失败只告警，不影响研究本身。
- **公司搜索接回界面**：`/api/companies/search` 此前没有任何前端调用方，用户必须手打精确 ticker。自选、持仓、监控规则三处代码输入框现在共用 `TickerSearchInput`（防抖补全，选中回填公司名）。
- **深度报告接回界面**：`/api/report/generate` 以答案动作条上的"深度报告"回归，直接下载 Markdown；对比轮不出该按钮（报告服务是单主体口径）。
- **邀请码接回界面**：注册强制要邀请码，而 `/api/auth/invite` 没有入口——owner 实际上无法邀请第二个人。设置页新增 owner 专属的生成/复制卡片。
- **冻结函数清理**：删除 `position_return_pct`（worker 另有实现）与 `parse_compact_amount`（`extract_numbers` 的 `RE_AMOUNT_UNIT` 已内联覆盖）。

## 2026-07-28 前端质感与对话连续性

- **深色模式破面修复**：`04-shell` / `06-research` 与被设置页复用的 `.auth-tabs` 里约 40 处写死的浅色（导航胶囊、示例公司卡、composer、会话日期分组、头像、订阅卡、思考文字高光）在深底上是白底白字。全部收敛到令牌，并补 `--glass-thumb` / `--glass-hover` / `--glass-thumb-shadow` 三个此前缺失的"玻璃上浮起一层"令牌。组件样式仍然零深色分支。
  登录页 `.auth-page` 是**刻意锁定浅色**（自带 `color-scheme: light` 与整套浅色令牌覆盖），本轮不动——它自洽，不属于破面。
- **页头叠印修复**：`.workspace-stage` 固定成单行网格，而资料库/设置返回的是「页头 + 内容」两个兄弟元素，两者被塞进同一格叠印，页面大标题被分段控件压掉一半（深浅两色都有）。改成 `grid-auto-rows: min-content`，用 `:has(> :only-child)` 保留研究台撑满高度的行为。
- **动效**：51 处 `transition ... linear` 改 `--ease-out`；补 `.nav-item` / `.segmented-item` / `.answer-action` / 会话项 / 追问 chip / 补全项的 `:active` 按压反馈；页面切换从 140ms 纯淡入改为 `--dur-3` 的淡入 + 6px 上移，页头与内容错开 60ms。全局 `prefers-reduced-motion` 块已覆盖关闭。
- **连续对话**：作答提示词此前固定只带最近 3 轮、每轮答案截断 300 字符，第 4 轮起用户说过的全部消失。改为按字符预算回看（最多 8 轮 / 总计 2400 字符，最近一轮 700 字、逐轮 ×0.68 衰减到 180 字下限），并保证按时间正序喂入。历史仍然只作指代线索，块头的数字禁令不变。
- **记忆沉淀条件**：`persist_company_memory_if_safe` 把 `citation_guard.ungrounded`（领域层明确定义为"soft 提示、不拦截"）当硬条件用，导致最该积累记忆的定性研究只要模型忘写来源号就永远存不下论点。改为只有硬失败（数字对不上、虚构来源号）才拦。
- **护栏认口径**：`verify_answer_numbers` 此前只问"这个数字是不是同维度桶里**某条**事实"，于是「净利率 45%」在毛利率恰好 45% 时照样 Pass——数字个个都对、说法全错，这是护栏长期的头号盲区。现在正文点名口径（毛利率/经营利润率/净利率、收入与利润增速、ROE/ROA、股息率、PE/PB）时，数字必须核**那条**口径；对不上同名事实、却正好等于另一条口径的真值时判 hard，原因写明"这个数字实际属于谁"。
  标签按包含匹配，所以「PE 中位」这类同口径衍生事实照样认下。误报闸：窗口里出现换时间（从/去年/同期/前值…）或换主体（同业/行业/可比/平均…）的词就整体退回原逻辑——登记表只登本公司当期，「同业净利率约 45%」「去年毛利率 25%」都不该按当期口径判死。其余情况一律走原匹配，不新增误报面。

## 2026-07-28 交互与界面复查（逐项实机验证）

- **固定定位被舞台劫持**：`.workspace-stage > *` 的进场动画用 `fill-mode: both`，动画结束后元素上永久留着一个单位矩阵 transform，舞台因此始终是 transform containing block——它内部所有 `position: fixed` 都不再相对视口。窄屏历史抽屉整体下坠 52px，顶栏与抽屉之间露出一条谁也没遮住的缝。`to` 本就等于静止态，不需要 forwards 保持，改 `backwards` 后 transform 彻底消失（实测 `getComputedStyle(...).transform === "none"`），视觉不变。
- **覆盖式抽屉不再透字**：窄屏抽屉用 `--glass-strong` 且漏配 `backdrop-filter`，背后的大标题与答案正文直接透上来，和会话列表叠成幽灵字。改为不透明纸面——玻璃适合顶栏那种薄薄一条、内容只是路过，一整屏要逐行读的列表必须自己挡住背景。同时补 `.sidebar-scrim`：覆盖式抽屉点外面就该收起来，宽屏（常驻栏）用 CSS 关掉这层。
- **历史答案能带走了**：`HistoryCard` 此前完全没有动作条，重开一条落库的研究会话，复制/导出/深度报告/重新生成会整条消失——结论一旦存下来反而带不走，与"研究历史"的存在意义相反。现在与实时作答一视同仁；`session_id` 传 `None`，深度报告按该轮问题与主体现取现做，不假装能复原当时上下文。
- **用户气泡两个主题下不再是两种设计**：气泡沿用 `--paper-sunken`，浅色下距画布只差一档极浅的蓝（#edf3fa vs #f4f8fd）形状看不出，深色下（#232326 vs #141416）却是清清楚楚一只气泡。单列 `--bubble-user`。
- **示例卡动作对齐**：`.company-card` 原来 `align-content: start` 把三行全顶到上边，问题只有一行的卡分隔线与箭头比邻居高出一整行。改 `grid-template-rows: auto 1fr auto`，同一排的动作永远排在同一条线上。
- **空态不再指使用户点不存在的东西**：七处空态共用同一句「完成上方操作后，内容会出现在这里。」，但通知与触发记录是系统跑出来的，上方没有任何操作。`empty_view` 增加 `hint` 参数，逐处说清"这里的内容从哪来"。
- **写死颜色清理**：`.primary-button.is-danger:active` 的 `#7a170f` 深色下没有对应值——hover 变亮、按下却掉进看不见的暗红，交互逻辑反了；补 `--danger-active`。设置页 sticky 保存条的渐变起点 `rgba(245,245,247,0)` 是浅色专用值，改 `transparent`。

## 后续增强（不改变单栈边界）

- 接入已签署商业协议的行情/财报适配器，并补充真实供应商 canary。
- 将更多一手公告/港交所披露解析接入 `echo-data`，保持 bitemporal 与来源 URL。
- 为研究历史增加报告导出与画像编辑页面；服务端能力需先在矩阵中从 pending 变为已验收。
- 公司档案已自动跨会话读写：只回放遮蔽数字后的定性线索，护栏通过后更新 Markdown 自动段与已核估值字段；后续补“自动段/手工段”在 UI 上的差异化审阅。
- 在预生产跑 RLS 双租户、备份恢复、外部源故障和 Worker 重启联合演练。
