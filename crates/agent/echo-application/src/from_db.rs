//! 持久化行 → 领域输入的映射（IO 层接线：`echo-db` ↔ `echo-domain`）。
//!
//! 编排层是唯一允许同时认识"数据库行"与"领域类型"的地方（CLAUDE.md 分层：echo-db 只管持久化、
//! echo-domain 只放纯规则，两者互不依赖，靠 echo-application 缝合）。这里把 `CompanyRow` /
//! `MarketRow` 折叠成一份**单一公司**的领域事实，喂给估值与护栏——决策面板与作答只吃这一家的
//! 数字，跨公司污染（"问苹果答腾讯"）在类型层就发生不了。
//!
//! 纯函数、无时钟无 IO：行由仓储在边界处取好后传入，这里只做形状转换，可脱离活库单测。

use crate::ResolvedCompany;
use echo_db::{CompanyRow, MarketRow};
use echo_domain::{Company, MarketSnapshot};

/// 行情快照：数值取自 `market_snapshots`，报价币种取自 `companies`（快照表不带币种，
/// 币种是公司身份的一部分——HKEX=HKD、美股=USD）。缺数即缺——不做占位兜底。
#[must_use]
pub fn market_snapshot_from_rows(company: &CompanyRow, market: &MarketRow) -> MarketSnapshot {
    MarketSnapshot {
        price: market.price,
        pe: market.pe,
        market_cap: market.market_cap,
        currency: Some(company.currency.clone()),
        change_percent: market.change_percent,
        dividend_yield: market.dividend_yield,
    }
}

/// 公司身份 + 估值会用到的行情数值折进一个 `Company`。`pb` 无对应列，留空（估值内核会据缺失
/// 自行退法，绝不臆造倍数）。`name_zh` 为空串时归一成 `None`，避免把占位空名当有效中文名。
#[must_use]
pub fn resolved_company_from_rows(
    company: &CompanyRow,
    market: Option<&MarketRow>,
) -> ResolvedCompany {
    let name_zh = if company.name_zh.trim().is_empty() {
        None
    } else {
        Some(company.name_zh.clone())
    };
    ResolvedCompany {
        ticker: company.ticker.clone(),
        name_zh,
        company: Company {
            sector: company.sector.clone(),
            price: market.and_then(|m| m.price),
            pe: market.and_then(|m| m.pe),
            pb: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn company_row() -> CompanyRow {
        CompanyRow {
            ticker: "0700.HK".into(),
            name_zh: "腾讯控股".into(),
            name_en: Some("Tencent".into()),
            sector: Some("Communication Services".into()),
            industry: Some("Interactive Media".into()),
            exchange: "HKEX".into(),
            currency: "HKD".into(),
            listing_status: "active".into(),
        }
    }

    fn market_row() -> MarketRow {
        MarketRow {
            ticker: "0700.HK".into(),
            price: Some(dec!(380.4)),
            change_percent: Some(dec!(1.2)),
            market_cap: Some(dec!(3500000000000)),
            pe: Some(dec!(18.5)),
            dividend_yield: Some(dec!(0.8)),
            source: Some("finnhub".into()),
            valid_time: chrono::Utc::now(),
        }
    }

    #[test]
    fn snapshot_takes_currency_from_company_not_market() {
        // market_snapshots 表没有币种列——币种必须来自 companies.currency，否则 HKD 报价会被
        // 误当 USD 参与估值/护栏比对。
        let snap = market_snapshot_from_rows(&company_row(), &market_row());
        assert_eq!(snap.currency.as_deref(), Some("HKD"));
        assert_eq!(snap.price, Some(dec!(380.4)));
        assert_eq!(snap.pe, Some(dec!(18.5)));
        assert_eq!(snap.market_cap, Some(dec!(3500000000000)));
        assert_eq!(snap.dividend_yield, Some(dec!(0.8)));
        assert!(snap.is_ok());
    }

    #[test]
    fn company_overlays_market_price_and_pe() {
        let resolved = resolved_company_from_rows(&company_row(), Some(&market_row()));
        assert_eq!(resolved.ticker, "0700.HK");
        assert_eq!(resolved.name_zh.as_deref(), Some("腾讯控股"));
        assert_eq!(
            resolved.company.sector.as_deref(),
            Some("Communication Services")
        );
        assert_eq!(resolved.company.price, Some(dec!(380.4)));
        assert_eq!(resolved.company.pe, Some(dec!(18.5)));
        // 无 pb 列 → 留空，不臆造。
        assert_eq!(resolved.company.pb, None);
    }

    #[test]
    fn missing_market_leaves_price_empty_not_zero() {
        // 缺行情即缺——不得用 0 冒充价格（会污染估值与仓位盈亏）。
        let resolved = resolved_company_from_rows(&company_row(), None);
        assert_eq!(resolved.company.price, None);
        assert_eq!(resolved.company.pe, None);
    }

    #[test]
    fn blank_name_normalizes_to_none() {
        let mut row = company_row();
        row.name_zh = "   ".into();
        let resolved = resolved_company_from_rows(&row, None);
        assert_eq!(resolved.name_zh, None);
    }
}
