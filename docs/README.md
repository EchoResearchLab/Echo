# 文档导航

- [PLAN.md](PLAN.md)：唯一计划与架构底账——结构迁移 vs 功能平价口径、不变量、发布门禁、生产闭环待办；交接给其他 AI 时先读这一份。
- [architecture/repository-layout.md](architecture/repository-layout.md)：分层目录（前端 / 后端 / agent / 数据库 / 数据源 / 内核）、依赖方向与新增代码归属。
- [architecture/system-overview.md](architecture/system-overview.md)：运行时拓扑、数据质量和租户边界。
- [architecture/conversation-research-engine.md](architecture/conversation-research-engine.md)：研究回答的意图、估值和事实护栏。
- [ui-design-system.md](ui-design-system.md)：Echo Editorial 视觉令牌、组件状态、动效与可访问性规范。
- [hk-financial-ingest.md](hk-financial-ingest.md)：HKEX 来源校验、金额单位归一化与安全写入命令。
- [qa/README.md](qa/README.md)：Cargo 与 Rust WebDriver 验收门禁、离线 QA 语料。

文档只描述当前架构；退役的 Node/Python/React 实现不再作为有效运行路径维护。功能是否平价以 PLAN.md 的逐区口径为准，不以“能编译”代替。
