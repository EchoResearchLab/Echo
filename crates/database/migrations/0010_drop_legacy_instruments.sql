-- 0010: companies 表垃圾行清理（破坏性，2026-07-20 经负责人确认删除）。
--
-- 删除对象（共 ~74 行）：
--   1. A 股遗留行（ticker ~ '\.(SS|SZ)$'，67 行）——A 股已于 v3 退场，这些行只会让
--      "美的集团"这类查询命中一行永远不再更新的死数据，而不是诚实的"未覆盖"。
--   2. 测试/归档垃圾行：TEST.TMP、TESTUS、TRPCTEST、
--      AAPL.US、name_en='Archived legacy instrument' 的行（MSFT/RKLB 等——真代码，
--      但行内容是垃圾；下次研究会经 ensureCompanyRow 用真实名称重新建档）、
--      已退市的 PTR（PetroChina ADR，2022 年退市）。
--
-- 可恢复性（红线 8）：每张受影响表的被删行先复制进 _archive_0010_* 归档表，
-- 迁移本身即备份；确认无误后可另行 DROP 归档表。
-- 用户资产不受影响：portfolio_positions / watchlist_prefs / watch_rules /
-- company_profiles / research_snapshots 均无 companies 外键，一行不动。
-- research_sessions（用户研究历史）只把 ticker 置 NULL（外键松开、正文保留），不删行。

CREATE TABLE _archive_0010_tickers AS
SELECT ticker FROM companies
WHERE ticker ~ '\.(SS|SZ)$'
   OR ticker IN ('TEST.TMP', 'TESTUS', 'TRPCTEST', 'AAPL.US', 'PTR')
   OR name_en = 'Archived legacy instrument';

-- 研究历史：保正文、松外键（先归档 id↔ticker 映射以便恢复）。
CREATE TABLE _archive_0010_research_session_tickers AS
SELECT id, ticker FROM research_sessions WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);
UPDATE research_sessions SET ticker = NULL WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);

-- 引用 companies(ticker) 的仓库/缓存表：归档后删除。
CREATE TABLE _archive_0010_market_snapshots AS
SELECT * FROM market_snapshots WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);
DELETE FROM market_snapshots WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);

CREATE TABLE _archive_0010_hk_financials AS
SELECT * FROM hk_financials WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);
DELETE FROM hk_financials WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);

CREATE TABLE _archive_0010_hk_filing_ingest_log AS
SELECT * FROM hk_filing_ingest_log WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);
DELETE FROM hk_filing_ingest_log WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);

CREATE TABLE _archive_0010_hk_buybacks AS
SELECT * FROM hk_buybacks WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);
DELETE FROM hk_buybacks WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);

CREATE TABLE _archive_0010_earnings_calendar AS
SELECT * FROM earnings_calendar WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);
DELETE FROM earnings_calendar WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);

CREATE TABLE _archive_0010_comp_peers AS
SELECT * FROM comp_peers WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);
DELETE FROM comp_peers WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);

CREATE TABLE _archive_0010_insider_activity AS
SELECT * FROM insider_activity WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);
DELETE FROM insider_activity WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);

CREATE TABLE _archive_0010_historical_valuation_points AS
SELECT * FROM historical_valuation_points WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);
DELETE FROM historical_valuation_points WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);

CREATE TABLE _archive_0010_historical_valuation AS
SELECT * FROM historical_valuation WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);
DELETE FROM historical_valuation WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);

CREATE TABLE _archive_0010_web_evidence AS
SELECT * FROM web_evidence WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);
DELETE FROM web_evidence WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);

CREATE TABLE _archive_0010_research_facts AS
SELECT * FROM research_facts WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);
DELETE FROM research_facts WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);

CREATE TABLE _archive_0010_research_questions AS
SELECT * FROM research_questions WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);
DELETE FROM research_questions WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);

CREATE TABLE _archive_0010_review_dates AS
SELECT * FROM review_dates WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);
DELETE FROM review_dates WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);

CREATE TABLE _archive_0010_company_details AS
SELECT * FROM company_details WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);
DELETE FROM company_details WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);

CREATE TABLE _archive_0010_companies AS
SELECT * FROM companies WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);
DELETE FROM companies WHERE ticker IN (SELECT ticker FROM _archive_0010_tickers);
