# 港股财报结构化 ingest

## 研究时的自动补救

研究主链读不到 `hk_financials` 时，`ResearchOrchestrator` 会把 `financial_statements` /
`ttm_eps` 等缺口交给数据端。数据端只在第一轮调用 HKEX 官方发现接口，选择最近 FY 业绩
公告 PDF：

- PDF 必须来自 `hkexnews.hk` / `hkex.com.hk`；
- 纯 Rust 提取文本，金额单位声明必须在同一行明确出现币种与单位；
- 收入、上期收入、净利润、上期净利润是最低成组要求；
- 解析结果仍必须经过本文既有的 `normalize_hk_financials` 与数量级质量门才可写库；
- 严格解析失败时只把官方公告作为 filing 证据返回，财务字段保持 `None`。

先从 HKEX 披露易发现最近两年的官方业绩公告：

```bash
cargo xtask hk-discover 0700.HK
```

输出包含公告原题、PDF URL、发布时间及解析后的 `period_end`/`period_type`/`period_label`。
股票代码只用于查询 HKEX 官方端点，不经过搜索引擎或二手数据源。

港股绝对金额只允许通过运维命令写入：

```bash
DATABASE_URL=postgres://... cargo xtask hk-ingest ./hk-results.json
```

命令会先执行数据库迁移，再校验港股代码、HKEX 披露易 HTTPS 来源、报告币种和原始金额单位，
把金额统一换算为绝对值后写入。公共 HTTP API 不开放写入口，避免普通登录用户污染公共财务表。

输入数字必须是公告表格里的**原始数字**，不能预先乘单位；EPS 是每股值，不乘单位。

```json
{
  "ticker": "0700.HK",
  "period_label": "2025 FY",
  "period_end": "2025-12-31",
  "period_type": "FY",
  "currency": "CNY",
  "source_unit": "百萬元",
  "revenue": "751766",
  "revenue_prior": "660257",
  "gross_profit": "427366",
  "gross_profit_prior": null,
  "operating_income": null,
  "operating_income_prior": null,
  "net_income": "228011",
  "net_income_prior": null,
  "net_income_attributable": null,
  "eps": "24.56",
  "operating_cash_flow": null,
  "cash_and_equivalents": null,
  "net_cash": null,
  "free_cash_flow": null,
  "source_title": "请替换为 HKEX 公告原题",
  "source_url": "https://www1.hkexnews.hk/path/to/original-announcement.pdf",
  "published_at": "2026-03-18T08:00:00Z"
}
```

支持的单位为元、千/千元、百万/百万元、十亿/十亿元及对应英文
`thousand`/`million`/`billion`。无法精确识别就拒绝，不能猜倍率。

读侧规则：

- `amounts_normalized=false` 的历史行只用于同一行内毛利率、净利率和增速，绝对金额不外传。
- `amounts_normalized=true` 的新行可进入展示和估值。
- EV/Sales 还要求报告币种与行情币种一致；没有 FX 时，CNY 财报不能直接除以 HKD 企业价值。

当前自动化边界：公告发现与期间解析已经迁入 Rust；PDF 文本抽取和三表行解析尚未接到
`hk-ingest`，因此发现结果不会自动写库。
