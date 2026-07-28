# Conversation research engine

`route_research_intent` 先用纯规则确定意图、深度与回答风格；`build_panel` 对同一家公司计算阶段感知估值区间和数据完备度；模型网关只负责自然语言表达。无 provider 时返回结构化事实和 `unavailable`，不会拼接假答案。

模型生成文本和用户草稿共用 `verify_answer_numbers` 数字护栏。来源段不扫描，币种不匹配不通过，符号翻转是硬失败，缺少实时事实就明确“未核到”。研究响应成功后 best-effort 写入 `research_sessions`，落库失败不吞掉本轮回答。

深度研究的长期事实、证伪线和通知由 PostgreSQL 工作区仓储承接；Worker 只调用这些仓储和纯领域规则，不在定时任务中复制估值或通知策略。

## 缺口驱动补救循环

`ResearchOrchestrator` 包在单次取数执行器 `ResearchService::assemble_core_facts` 外：

1. 按意图评估行情、财报、年化 EPS、营收/股本、历史估值、同业、公告与最终估值缺口。
2. 把缺口作为 `FactRecoveryRequest` 交给数据端，应用层不点名供应商。
3. 数据端按市场换源：美股主财务失败后查 SEC Company Facts；港股查最近 HKEX FY 官方 PDF，经文本提取、明确单位识别、数量级质量门后才写 `hk_financials`。
4. 合并只填 `None`，唯一允许的覆盖是把明确标为非年化的单季/中报 EPS 升级为 TTM/FY；缺口集合没有变化就提前停止，硬上限为两轮。
5. 最终估值仍失败但有同业锚点时，提示词进入 `peer_comparison_only`，只给同业倍数分布与可比性限制；一手财务仍缺则进入 `qualitative_only`。

每轮请求、剩余缺口、命中来源与最终降级形态写入研究会话 `data_sources.recovery`，便于回放与生产 canary。

## 跨会话公司记忆

`company_profiles` 是用户 + ticker 维度的长期 Markdown 档案。每次新研究会话都会读取 thesis、bull/bear、监控项、证伪线与 `profile_md`，但只作为“不可信的定性线索”附在本轮实时事实之后：

- 所有阿拉伯/中文数字在进入提示词前遮蔽，旧价格和旧估值不能成为本轮数字来源。
- 本轮数字仍只来自 `FactsRegistry`；记忆不进入登记表。
- 只有答案通过数字与引用硬护栏后，才 best-effort 更新 `## 自动研究记忆`；手工 Markdown 段保留。
- 已核估值同时写结构化档案字段供 UI 展示，但下一轮仍重新取数，不拿档案估值参与计算。
