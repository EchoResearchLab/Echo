use chrono::{Datelike, Utc};
use echo_data::{EmailService, HistoricalValuationService, QuoteService, looks_like_email};
use echo_db::{
    AuthRepository, NewNotification, NotificationsRepository, OperationsRepository, Pool,
    PortfolioRepository,
};
use rust_decimal::Decimal;
use tracing::warn;

use crate::schedule::JobKind;

#[derive(Debug, thiserror::Error)]
pub enum ActivityError {
    #[error(transparent)]
    Database(#[from] echo_db::DbError),
    #[error(transparent)]
    Quote(#[from] echo_data::QuoteError),
    #[error(transparent)]
    HistoricalValuation(#[from] echo_data::HistoricalValuationError),
}

pub struct Activities {
    pool: Pool,
    quotes: QuoteService,
    historical_valuation: HistoricalValuationService,
    email: Option<EmailService>,
}

impl Activities {
    pub async fn new(
        pool: Pool,
        data_sources: echo_config::DataSourceConfig,
        email: Option<echo_config::EmailConfig>,
    ) -> Result<Self, ActivityError> {
        let email = email.and_then(|config| match EmailService::new(&config) {
            Ok(service) => Some(service),
            Err(error) => {
                warn!(error = %error, "SMTP 配置无效，简报邮件通道禁用，仅保留站内通知");
                None
            }
        });
        Ok(Self {
            quotes: QuoteService::new(pool.clone(), data_sources.clone())?,
            historical_valuation: HistoricalValuationService::new(pool.clone(), data_sources)?,
            email,
            pool,
        })
    }

    pub async fn run(&self, job: JobKind) -> Result<String, ActivityError> {
        match job {
            JobKind::PremarketDigest => self.digest("premarket").await,
            JobKind::AfterhoursDigest => self.digest("afterhours").await,
            JobKind::MarketRefresh => self.refresh_market().await,
            JobKind::PortfolioSnapshot => self.capture_portfolios().await,
            JobKind::FalsifierCheck => self.check_falsifiers().await,
            JobKind::EarningsReview => self.review_earnings().await,
            JobKind::PositionAlert => self.check_positions().await,
            JobKind::ReviewReminder => self.review_reminders().await,
        }
    }

    /// 移动幅度超过此百分点才算简报里的"异动"，避免噪音行情把简报灌满。
    const DIGEST_MOVER_THRESHOLD: i64 = 3;

    async fn digest(&self, slot: &str) -> Result<String, ActivityError> {
        let operations = OperationsRepository::new(&self.pool);
        let portfolios = PortfolioRepository::new(&self.pool);
        let users = operations.user_ids().await?;
        let mut emitted = 0usize;
        let date = Utc::now().date_naive();
        let mover_threshold = Decimal::from(Self::DIGEST_MOVER_THRESHOLD);
        for user_id in &users {
            let rule_count = operations.active_rules(user_id).await?.len();
            let triggered_count = operations
                .recently_triggered_rule_count(user_id, 12)
                .await?;
            let positions = portfolios.list(user_id).await?;
            let mut movers = Vec::new();
            for position in &positions {
                if let Some(change) = operations
                    .latest_market(&position.ticker)
                    .await?
                    .and_then(|market| market.change_percent)
                {
                    if change.abs() >= mover_threshold {
                        let sign = if change.is_sign_positive() { "+" } else { "" };
                        movers.push(format!("{}{sign}{}%", position.ticker, change.round_dp(1)));
                    }
                }
            }
            let title = if slot == "premarket" {
                "盘前研究速报"
            } else {
                "盘后研究速报"
            };
            let mut body = format!(
                "持仓 {} 个，其中 {} 个日内异动（>{}%）",
                positions.len(),
                movers.len(),
                Self::DIGEST_MOVER_THRESHOLD
            );
            if !movers.is_empty() {
                body.push('：');
                body.push_str(&movers.join("、"));
            }
            body.push_str(&format!(
                "。有效监控条件 {rule_count} 条，本轮触发 {triggered_count} 条。"
            ));
            let key = format!("digest:{slot}:{date}");
            let inserted = self
                .notify(
                    user_id,
                    &NewNotification {
                        kind: "event_digest",
                        title,
                        body: &body,
                        ticker: None,
                        payload: None,
                        dedupe_key: Some(&key),
                        dedupe_window_hours: 12,
                    },
                )
                .await?;
            if inserted {
                emitted += 1;
                self.send_digest_email(user_id, title, &body).await;
            }
        }
        Ok(format!("用户={}，推送={emitted}", users.len()))
    }

    /// 简报邮件是站内通知的镜像通道：只有通知已经过偏好/免打扰/去重咽喉真正落库
    /// （调用方传入的 `inserted` 已隐含这一点），才尝试同步发信；未配置 SMTP 或账号
    /// 不是邮箱形态时静默跳过，绝不因为邮件通道缺失而影响站内通知本身。
    async fn send_digest_email(&self, user_id: &str, title: &str, body: &str) {
        let Some(email) = &self.email else { return };
        let user = match AuthRepository::new(&self.pool).user_by_id(user_id).await {
            Ok(Some(user)) => user,
            Ok(None) => return,
            Err(error) => {
                warn!(user_id, error = %error, "读取用户账号失败，跳过简报邮件");
                return;
            }
        };
        if !looks_like_email(&user.username) {
            return;
        }
        if let Err(error) = email.send(&user.username, title, body).await {
            warn!(user_id, error = %error, "简报邮件发送失败，站内通知已落库");
        }
    }

    async fn refresh_market(&self) -> Result<String, ActivityError> {
        let tickers = OperationsRepository::new(&self.pool)
            .tracked_tickers()
            .await?;
        let mut refreshed = 0usize;
        let mut failed = Vec::new();
        for ticker in &tickers {
            match self.quotes.refresh(ticker).await {
                Ok(result) if result.quote.price.is_some() => refreshed += 1,
                Ok(_) => failed.push(format!("{ticker}:missing")),
                Err(error) => failed.push(format!("{ticker}:{error}")),
            }
        }
        Ok(format!(
            "总数={}，刷新={refreshed}，失败={}{}",
            tickers.len(),
            failed.len(),
            if failed.is_empty() {
                String::new()
            } else {
                format!(" [{}]", failed.join(", "))
            }
        ))
    }

    async fn capture_portfolios(&self) -> Result<String, ActivityError> {
        let hkd_usd = self
            .quotes
            .refresh("HKDUSD=X")
            .await
            .ok()
            .and_then(|result| result.quote.price);
        let operations = OperationsRepository::new(&self.pool);
        let users = operations.user_ids().await?;
        let date = Utc::now().date_naive();
        let mut saved = 0usize;
        let mut gaps = 0usize;
        let mut cost_gaps = 0usize;
        for user_id in &users {
            let result = operations
                .capture_portfolio_snapshot(user_id, date, hkd_usd)
                .await?;
            if result.position_count == 0 {
                continue;
            }
            if result.missing_price == 0 && result.missing_fx == 0 && result.missing_cost == 0 {
                saved += 1;
            } else if result.missing_price == 0 && result.missing_fx == 0 {
                // 行情齐、只差用户没填成本价——这是可以让用户自己补上的缺口，
                // 和取不到行情的系统性缺口分开报，否则运维看不出该找谁。
                cost_gaps += 1;
            } else {
                gaps += 1;
            }
        }
        Ok(format!(
            "组合快照={saved}，因缺价/汇率跳过={gaps}，因未填成本价跳过={cost_gaps}"
        ))
    }

    async fn check_falsifiers(&self) -> Result<String, ActivityError> {
        let operations = OperationsRepository::new(&self.pool);
        let users = operations.user_ids().await?;
        let mut checked = 0usize;
        let mut triggered = 0usize;
        for user_id in &users {
            for rule in operations.active_rules(user_id).await? {
                checked += 1;
                let current = if matches!(rule.kind.as_str(), "price_below" | "price_above") {
                    operations
                        .latest_market(&rule.ticker)
                        .await?
                        .and_then(|market| market.price)
                } else if matches!(
                    rule.kind.as_str(),
                    "fundamental_below" | "fundamental_above"
                ) {
                    match rule.metric.as_deref() {
                        Some(metric) => {
                            operations
                                .latest_fundamental_metric(&rule.ticker, metric)
                                .await?
                        }
                        None => None,
                    }
                } else if matches!(
                    rule.kind.as_str(),
                    "valuation_percentile_below" | "valuation_percentile_above"
                ) {
                    self.historical_valuation
                        .load(&rule.ticker)
                        .await
                        .and_then(|summary| summary.percentile)
                } else {
                    None
                };
                let Some(current) = current else { continue };
                let hit = match rule.kind.as_str() {
                    "price_below" | "fundamental_below" | "valuation_percentile_below" => {
                        current <= rule.threshold
                    }
                    "price_above" | "fundamental_above" | "valuation_percentile_above" => {
                        current >= rule.threshold
                    }
                    _ => false,
                };
                if !hit {
                    continue;
                }
                let title = format!("{} 证伪条件触线", rule.ticker);
                let body = format!(
                    "{}（当前 {}，阈值 {}）",
                    rule.label.as_deref().unwrap_or("监控条件"),
                    current.normalize(),
                    rule.threshold.normalize()
                );
                let key = format!("falsifier:{}", rule.id);
                if self
                    .notify(
                        user_id,
                        &NewNotification {
                            kind: "falsify_alert",
                            title: &title,
                            body: &body,
                            ticker: Some(&rule.ticker),
                            payload: None,
                            dedupe_key: Some(&key),
                            dedupe_window_hours: 12,
                        },
                    )
                    .await?
                {
                    triggered += 1;
                }
                operations.mark_rule_triggered(user_id, rule.id).await?;
            }
        }
        Ok(format!("核对={checked}，触线={triggered}"))
    }

    async fn review_earnings(&self) -> Result<String, ActivityError> {
        let operations = OperationsRepository::new(&self.pool);
        let users = operations.user_ids().await?;
        let candidates = operations.earnings_candidates().await?;
        let mut emitted = 0usize;
        for candidate in &candidates {
            let summary = format!(
                "业绩事实已更新：EPS actual {}，estimate {}。",
                decimal_or_gap(candidate.last_eps_actual),
                decimal_or_gap(candidate.last_eps_estimate)
            );
            for user_id in &users {
                if let Some(company_name) =
                    operations.profile_name(user_id, &candidate.ticker).await?
                {
                    operations
                        .append_earnings_event(user_id, candidate, &summary)
                        .await?;
                    let title = format!("{company_name} 业绩闭环");
                    let key = format!(
                        "earnings:{}:{}",
                        candidate.ticker,
                        candidate.last_date.as_deref().unwrap_or("unknown")
                    );
                    emitted += self
                        .notify(
                            user_id,
                            &NewNotification {
                                kind: "earnings_review",
                                title: &title,
                                body: &summary,
                                ticker: Some(&candidate.ticker),
                                payload: None,
                                dedupe_key: Some(&key),
                                dedupe_window_hours: 12,
                            },
                        )
                        .await? as usize;
                }
                for rule in operations.active_rules(user_id).await? {
                    if rule.kind != "event_earnings" || rule.ticker != candidate.ticker {
                        continue;
                    }
                    let title = format!("{} 触发事件监控", rule.ticker);
                    let key = format!(
                        "event_earnings:{}:{}",
                        rule.id,
                        candidate.last_date.as_deref().unwrap_or("unknown")
                    );
                    emitted += self
                        .notify(
                            user_id,
                            &NewNotification {
                                kind: "falsify_alert",
                                title: &title,
                                body: &summary,
                                ticker: Some(&candidate.ticker),
                                payload: None,
                                dedupe_key: Some(&key),
                                dedupe_window_hours: 24,
                            },
                        )
                        .await? as usize;
                    operations.mark_rule_triggered(user_id, rule.id).await?;
                }
            }
        }
        Ok(format!("候选={}，闭环推送={emitted}", candidates.len()))
    }

    async fn check_positions(&self) -> Result<String, ActivityError> {
        let operations = OperationsRepository::new(&self.pool);
        let users = operations.user_ids().await?;
        let mut checked = 0usize;
        let mut alerts = 0usize;
        for user_id in &users {
            for position in PortfolioRepository::new(&self.pool).list(user_id).await? {
                checked += 1;
                let Some(price) = operations
                    .latest_market(&position.ticker)
                    .await?
                    .and_then(|market| market.price)
                else {
                    continue;
                };
                if position
                    .stop_loss
                    .is_some_and(|threshold| price <= threshold)
                {
                    let threshold = position.stop_loss.expect("checked some");
                    alerts += self
                        .position_notification(user_id, &position.ticker, "止损", price, threshold)
                        .await? as usize;
                }
                if position
                    .take_profit
                    .is_some_and(|threshold| price >= threshold)
                {
                    let threshold = position.take_profit.expect("checked some");
                    alerts += self
                        .position_notification(user_id, &position.ticker, "止盈", price, threshold)
                        .await? as usize;
                }
                if let Some(cost) = position.avg_cost.filter(|cost| !cost.is_zero()) {
                    let drawdown = (price - cost) / cost * Decimal::from(100);
                    if drawdown <= Decimal::from(-15) {
                        let title = format!("{} 大幅回撤", position.ticker);
                        let body = format!(
                            "现价 {}，成本 {}，回撤 {}%",
                            price.normalize(),
                            cost.normalize(),
                            drawdown.round_dp(1).normalize()
                        );
                        let bucket = (drawdown / Decimal::from(5)).floor() * Decimal::from(5);
                        let key = format!(
                            "position:drawdown:{}:{}",
                            position.ticker,
                            bucket.normalize()
                        );
                        alerts += self
                            .notify(
                                user_id,
                                &NewNotification {
                                    kind: "position_alert",
                                    title: &title,
                                    body: &body,
                                    ticker: Some(&position.ticker),
                                    payload: None,
                                    dedupe_key: Some(&key),
                                    dedupe_window_hours: 12,
                                },
                            )
                            .await? as usize;
                    }
                }
            }
        }
        Ok(format!("持仓={checked}，告警={alerts}"))
    }

    async fn position_notification(
        &self,
        user_id: &str,
        ticker: &str,
        label: &str,
        price: Decimal,
        threshold: Decimal,
    ) -> Result<bool, ActivityError> {
        let title = format!("{ticker} 触及{label}线");
        let body = format!(
            "现价 {}，{}线 {}",
            price.normalize(),
            label,
            threshold.normalize()
        );
        let key = format!("position:{label}:{ticker}");
        self.notify(
            user_id,
            &NewNotification {
                kind: "position_alert",
                title: &title,
                body: &body,
                ticker: Some(ticker),
                payload: None,
                dedupe_key: Some(&key),
                dedupe_window_hours: 12,
            },
        )
        .await
    }

    async fn review_reminders(&self) -> Result<String, ActivityError> {
        let operations = OperationsRepository::new(&self.pool);
        let users = operations.user_ids().await?;
        let now = Utc::now();
        let mut emitted = 0usize;
        for user_id in &users {
            for profile in operations.reminder_profiles(user_id).await? {
                let days = now.signed_duration_since(profile.updated_at).num_days();
                let company = profile.company_name.as_deref().unwrap_or(&profile.ticker);
                let title = format!("{company} 研究已 {days} 天未更新");
                let body = format!(
                    "上次更新于 {}，建议复盘当前判断是否仍然成立",
                    profile.updated_at.date_naive()
                );
                let key = format!(
                    "review:{}:{:04}-{:02}",
                    profile.ticker,
                    now.year(),
                    now.month()
                );
                emitted += self
                    .notify(
                        user_id,
                        &NewNotification {
                            kind: "review_reminder",
                            title: &title,
                            body: &body,
                            ticker: Some(&profile.ticker),
                            payload: None,
                            dedupe_key: Some(&key),
                            dedupe_window_hours: 168,
                        },
                    )
                    .await? as usize;
            }
        }
        Ok(format!("复盘提醒={emitted}"))
    }

    async fn notify(
        &self,
        user_id: &str,
        notification: &NewNotification<'_>,
    ) -> Result<bool, ActivityError> {
        let inserted = NotificationsRepository::new(&self.pool)
            .insert(user_id, notification)
            .await?;
        Ok(inserted.is_some())
    }
}

fn decimal_or_gap(value: Option<Decimal>) -> String {
    value
        .map(|value| value.normalize().to_string())
        .unwrap_or_else(|| "未核到".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 活库集成：对真实开发库跑一遍简报/证伪巡检/业绩复盘，证明估值分位/事件触发规则种类与
    /// 真实简报内容生成不会在真数据上崩掉，且落库的通知走的是同一条偏好/免打扰/去重咽喉。
    /// 默认 `#[ignore]`，只在配了 DATABASE_URL 时手动跑：`cargo test -p echo-worker -- --ignored`。
    #[tokio::test]
    #[ignore = "需要活库 DATABASE_URL"]
    async fn live_digest_and_rule_checks_run_against_real_data() {
        let Ok(url) = std::env::var("DATABASE_URL") else {
            return;
        };
        let pool = echo_db::connect(&url, 2).await.expect("connect");
        let activities = Activities::new(pool, echo_config::DataSourceConfig::default(), None)
            .await
            .expect("build activities");

        let digest = activities
            .run(JobKind::PremarketDigest)
            .await
            .expect("digest runs against real users/portfolios/rules");
        assert!(digest.contains("用户="), "digest={digest}");

        let falsifiers = activities.run(JobKind::FalsifierCheck).await.expect(
            "falsifier check evaluates every active rule kind including valuation_percentile_*",
        );
        assert!(falsifiers.contains("核对="), "falsifiers={falsifiers}");

        let earnings = activities
            .run(JobKind::EarningsReview)
            .await
            .expect("earnings review also sweeps event_earnings rules");
        assert!(earnings.contains("候选="), "earnings={earnings}");
    }
}
