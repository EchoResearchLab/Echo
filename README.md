# Echo Research

**证据优先的美股 / 港股投资研究台。** 每一个数字可溯源、每一处缺数诚实标注、每一段生成文本过数字护栏——目标是成为最可信的投资研究平台，不提供买卖指令。

```text
Leptos/WASM 前端 ──▶ axum HTTP/SSE ──▶ echo-application 研究用例
                                            │
                        echo-domain 纯规则（意图路由 · 定点估值 · 数字护栏）
                                            │
              ┌─────────────────────────────┼─────────────────────────────┐
       echo-data 供应商路由           echo-db (sqlx + RLS)          echo-worker 后台活动
   （授权门 · 质量门 · 熔断）        （多租户强制隔离）           （行情刷新 · 复盘 · 证伪巡检）
```

全仓库只有一个工程入口：**Cargo**。没有 Node、没有 Python、没有手写 JS。

## 为什么可信

- **定点金融内核**：金额、股数、比率、估值全程 `rust_decimal::Decimal` + PostgreSQL NUMERIC，二进制浮点只允许出现在展示边界。`finance-core` 的估值不变量有独立测试。
- **数字护栏（fact guard）**：模型生成的每个数字都与已核事实登记表逐一比对，软失败标注、硬失败拦截——不让模型"顺口报数"。
- **诚实缺数**：拿不到的数据就是 `None` / "未核到"，永不用 0 占位、陈旧值回填或跨公司混数。
- **意图路由回归**：275 条语料固化在 `docs/qa/fixtures/`，估值 / 利润质量 / 护城河 / 证伪 / 对比等意图路由有离线基线，防止静默回退。
- **供应商合规**：数据源带授权元数据；商用模式（`ECHO_COMMERCIAL_MODE=1`）会把无商业授权的免费源整体从路由剔除，绝不悄悄兜底。
- **多租户 RLS**：私有数据同时经应用层用户过滤与 PostgreSQL 强制行级安全，双保险。

## 当前能力（诚实口径）

任意合法美股 / 港股代码或公司名可直接研究：公司解析（DB → 别名 → FMP 搜索 → 行情探活）、实时行情（Finnhub → Yahoo 降级链）、美股三表基本面（FMP）、意图路由、阶段感知定点估值（盈利 / 亏损成长分段）、模型作答 + 数字护栏、研究会话落库。自选、持仓、通知偏好、邀请制注册可用。

结构上已是 Cargo 单栈；功能平价与生产闭环仍在推进，逐区口径与待办见 [docs/PLAN.md](docs/PLAN.md)。"能编译 / 门禁绿"在本仓库不等于"能力完成"。

## 本地运行

需要 Rust 1.85、[Trunk](https://trunkrs.dev)（WASM 前端）和 PostgreSQL。未配 `DATABASE_URL` 时 API 仍可跑纯核研究路径，依赖库的功能会诚实返回不可用。

```bash
cp .env.example .env        # 按需填模型与数据源 key（只在服务端读取）
export DATABASE_URL=postgresql://postgres:postgres@127.0.0.1:5432/echo_dev
cargo xtask migrate

cargo run -p echo-api       # 终端一：API   → http://127.0.0.1:4180
cargo xtask web             # 终端二：前端  → http://127.0.0.1:5191（/api 自动代理）
cargo run -p echo-worker    # 终端三：后台活动
```

创建首个 owner：

```bash
ECHO_BOOTSTRAP_EMAIL=owner@example.com \
ECHO_BOOTSTRAP_PASSWORD='use-a-long-password' \
cargo xtask bootstrap-owner
```

## 验收门禁

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check -p echo-web --target wasm32-unknown-unknown
cargo xtask web

cargo xtask e2e   # 已启动 API、Trunk 和 chromedriver/geckodriver 时
```

离线研究质量回归：`cargo test -p echo-domain --test intent_routing_corpus`。
迁移 `0001`–`0010` 的 SHA-256 已冻结（`docs/qa/fixtures/migration-checksums.json`），新变更只加 `0011+`。

## 仓库结构

| 目录 | 职责 |
| --- | --- |
| `crates/frontend/echo-web` | 前端：Leptos/WASM 单页 |
| `crates/backend/echo-api` | 后端：axum HTTP/SSE 边界与认证 |
| `crates/backend/echo-worker` | 后端：可恢复后台活动（九类 cron） |
| `crates/agent/echo-application` | 研究智能体：取数 → 估值 → 生成 → 护栏 → 落库 |
| `crates/database/echo-db` | 唯一数据库入口：sqlx 仓储、迁移、RLS、通知咽喉 |
| `crates/datasource/echo-data` | 唯一外部供应商入口：授权门、质量门、熔断 |
| `crates/core/echo-domain` | 纯规则：意图路由、估值、财务衍生、数字护栏 |
| `crates/core/finance-core` | 定点金融算术内核（Money / Decimal 不变量） |
| `crates/core/echo-contracts` | HTTP 与 WASM 共用的 serde 契约 |
| `crates/platform/*` | 工程底座：环境配置、tracing/OTLP |
| `crates/qa/echo-e2e` | Rust WebDriver 浏览器验收 |
| `docs/` | 计划与架构底账（PLAN）、架构说明、QA 语料 |

## 文档

- [docs/PLAN.md](docs/PLAN.md) —— 唯一计划与架构底账（口径、不变量、门禁、生产闭环待办）
- [docs/architecture/](docs/architecture) —— 系统与研究引擎架构
