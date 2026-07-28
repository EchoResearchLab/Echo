# Echo Research 开发约束

- 用中文回答。
- Cargo 是唯一工程入口；禁止新增 Node、Python、手写 TypeScript/JavaScript 业务实现。
- `docs/PLAN.md` 是唯一计划与架构底账。

## 分层

`crates/` 的第一层就是分层，目录名说明职责，包名保持 `echo-*`（详见
`docs/architecture/repository-layout.md`）：

| 目录 | 职责 |
| --- | --- |
| `crates/frontend/` | Leptos/WASM UI，只放界面 |
| `crates/backend/` | axum HTTP/SSE 边界与后台调度，不复制数字逻辑 |
| `crates/agent/` | 研究智能体：编排、提示词、模型网关、研究记忆 |
| `crates/database/` | 唯一数据库入口（含 `migrations/`） |
| `crates/datasource/` | 唯一外部供应商入口 |
| `crates/core/` | 纯规则、定点算术、共用契约；不碰 IO |
| `crates/platform/` | 环境配置与观测 |
| `crates/qa/` | 浏览器验收 |

## 不变量

- 所有金额、股数、比率、估值使用 PostgreSQL NUMERIC 与 `rust_decimal::Decimal`；缺失数据用 `Option` 表示，禁止 0 占位或跨公司混数。
- 私有仓储必须同时经过应用层用户过滤和 PostgreSQL 强制 RLS；通知必须经过偏好、免打扰和去重咽喉。
- 商用模式只允许授权元数据明确允许商用的数据源。
- 研究以对话为中心：主体由服务端从问题文本识别，界面不把它当成用户要先填对的字段。

## 交付门禁

```bash
cargo xtask release   # 冻结检查 + fmt + clippy -D warnings + test + Trunk release
cargo check -p echo-web --target wasm32-unknown-unknown
cargo xtask e2e       # 有 WebDriver 时
```

冻结检查（`cargo xtask frozen`）拦本仓库的头号缺陷——**写好了没人调**：活表必须有 Rust
读写、API 路由必须有前端调用方，豁免须登记在 `docs/qa/frozen-registry.json` 并写明去向。
