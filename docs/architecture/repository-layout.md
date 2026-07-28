# 仓库结构

`crates/` 的第一层就是**分层**：想知道"前端在哪、后端在哪、数据库在哪、agent 在哪"，
`ls crates/` 就是答案，不需要先认识十二个 crate 的名字。

```text
crates/
  frontend/                 前端
    echo-web/               Leptos/WASM 单页（唯一 UI）

  backend/                  后端
    echo-api/               axum HTTP/SSE 边界与认证
    echo-worker/            后台作业与可恢复调度

  agent/                    研究智能体
    echo-application/       编排 · 提示词 · 模型网关 · 研究记忆 · 护栏调用

  database/                 数据库
    echo-db/                sqlx 仓储、多租户 RLS、通知咽喉
    migrations/             PostgreSQL SQL，由 echo-db 编译进二进制

  datasource/               外部数据
    echo-data/              供应商路由 · 授权门 · 质量门 · 熔断（唯一出网口）

  core/                     纯内核（不碰 IO，可被任何层依赖）
    echo-domain/            意图路由 · 估值 · 数字护栏 · 财务衍生
    finance-core/           定点金融算术（Money / Decimal 不变量）
    echo-contracts/         HTTP 与 WASM 共用的 serde 契约

  platform/                 工程底座
    echo-config/            服务端环境配置唯一入口
    echo-observability/     tracing / OTLP 初始化

  qa/                       验收
    echo-e2e/               Rust WebDriver 浏览器验收

xtask/                      唯一工程任务入口（check / web / e2e / migrate / frozen）
docs/                       计划底账、架构说明、设计系统、QA 语料
```

## 依赖方向

只能向下，且有两个"唯一入口"是硬约束：

```text
frontend ──▶ backend ──▶ agent ──▶ core
                 │         │
                 └────┬────┘
                      ▼
              database   datasource
             （唯一持久化）（唯一出网）
```

- `core` 不做任何 IO，也不知道上面有谁。
- `agent` 只组织用例，不自己写 SQL、不自己发 HTTP。
- `backend` 只做边界与调度，绝不复制一份数字逻辑。
- `database` 是唯一持久化入口，`datasource` 是唯一外部取数入口——绕过任何一个都算违规。

金额、股数、比率、估值全程 `rust_decimal::Decimal` + PostgreSQL NUMERIC；二进制浮点
只允许出现在展示边界。

## 目录名与包名

目录名说"这是哪一层"，包名保持 `echo-*` 不变（`echo-web`、`echo-api`、`echo-application`…）。
所以 `cargo run -p echo-api`、`use echo_domain::…` 全部照旧——这次重组只动了文件位置，
没有改任何代码引用。
