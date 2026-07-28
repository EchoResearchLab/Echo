//! 简报邮件发信——唯一外部邮件供应商入口（SMTP）。
//!
//! 调用方（`echo-worker`）必须先经 `NotificationsRepository::insert` 唯一咽喉
//! （偏好/免打扰/去重）拿到已落库的通知，再决定是否同步发信；本模块只管"怎么发"，
//! 不做发不发的策略判断。未配置 SMTP 时诚实返回 [`EmailError::NotConfigured`]，
//! 调用方按此静默降级为"仅站内通知"，不伪造发信成功。

use echo_config::EmailConfig;
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

#[derive(Debug, thiserror::Error)]
pub enum EmailError {
    #[error("SMTP 未配置")]
    NotConfigured,
    #[error("收件地址无效：{0}")]
    Address(#[from] lettre::address::AddressError),
    #[error("邮件构造失败：{0}")]
    Build(#[from] lettre::error::Error),
    #[error("SMTP 传输失败：{0}")]
    Transport(#[from] lettre::transport::smtp::Error),
}

#[derive(Clone)]
pub struct EmailService {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from_address: String,
}

impl EmailService {
    pub fn new(config: &EmailConfig) -> Result<Self, EmailError> {
        let credentials = Credentials::new(config.smtp_user.clone(), config.smtp_pass.clone());
        let transport = AsyncSmtpTransport::<Tokio1Executor>::relay(&config.smtp_host)?
            .port(config.smtp_port)
            .credentials(credentials)
            .build();
        Ok(Self {
            transport,
            from_address: config.from_address.clone(),
        })
    }

    pub async fn send(&self, to: &str, subject: &str, body: &str) -> Result<(), EmailError> {
        let message = Message::builder()
            .from(self.from_address.parse()?)
            .to(to.parse()?)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_string())?;
        self.transport.send(message).await?;
        Ok(())
    }
}

/// 账号标识（多数场景下即登录用户名）形似邮箱才当作收件地址——绝不把本地账号 id
/// 当邮箱发信（诚实缺数：没有邮箱就只有站内通知）。
#[must_use]
pub fn looks_like_email(account: &str) -> bool {
    account
        .split_once('@')
        .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_email_shaped_accounts() {
        assert!(looks_like_email("user@example.com"));
        assert!(!looks_like_email("local-account"));
        assert!(!looks_like_email("user@localhost"));
        assert!(!looks_like_email("@example.com"));
    }
}
