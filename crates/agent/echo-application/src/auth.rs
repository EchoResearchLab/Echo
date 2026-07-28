//! 认证用例：兼容旧栈 `s1$<salt hex>$<scrypt hex>` 密码格式，承接邀请注册和不透明 Cookie 会话。

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration, Utc};
use echo_contracts::{PublicUser, UserRole};
use echo_db::{AuthRepository, DbError, NewUser, Pool, UserRow};
use rand::RngCore;
use rand::rngs::OsRng;
use scrypt::{Params, scrypt};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;
use subtle::ConstantTimeEq;

const SESSION_DAYS: i64 = 30;
const OWNER_USER_ID: &str = "local";
const DUMMY_PASSWORD: &str = "echo-invalid-login-sentinel";
const DUMMY_SALT: [u8; 16] = [0; 16];

static DUMMY_PASSWORD_HASH: LazyLock<String> =
    LazyLock::new(|| hash_password_with_salt(DUMMY_PASSWORD, &DUMMY_SALT).expect("valid params"));

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("口令至少 8 位")]
    PasswordTooShort,
    #[error("请输入有效邮箱")]
    InvalidAccount,
    #[error("邮箱已被使用")]
    UsernameTaken,
    #[error("邀请码无效或已被使用")]
    InvalidInvite,
    #[error("用户名或密码不对")]
    InvalidCredentials,
    #[error("owner 已存在")]
    OwnerExists,
    #[error("数据库错误: {0}")]
    Database(#[from] DbError),
    #[error("密码计算任务失败")]
    PasswordTask,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub user: PublicUser,
    pub token: String,
}

pub struct AuthService<'a> {
    pool: &'a Pool,
}

impl<'a> AuthService<'a> {
    #[must_use]
    pub fn new(pool: &'a Pool) -> Self {
        Self { pool }
    }

    pub async fn user_count(&self) -> Result<i64, AuthError> {
        Ok(AuthRepository::new(self.pool).user_count().await?)
    }

    pub async fn local_owner(&self, id: &str) -> Result<PublicUser, AuthError> {
        Ok(public_user(
            AuthRepository::new(self.pool).ensure_local_user(id).await?,
        ))
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<Session, AuthError> {
        let repository = AuthRepository::new(self.pool);
        let user = repository.user_by_username(username).await?;
        let stored = user
            .as_ref()
            .map_or(DUMMY_PASSWORD_HASH.as_str(), |user| user.pass_hash.as_str());
        let valid = verify_password(password, stored).await;
        let user = user
            .filter(|_| valid)
            .ok_or(AuthError::InvalidCredentials)?;
        let token = self.create_session(&user.id).await?;
        repository.touch_last_login(&user.id).await?;
        Ok(Session {
            user: public_user(user),
            token,
        })
    }

    pub async fn register(
        &self,
        invite: &str,
        username: &str,
        password: &str,
        display_name: Option<String>,
    ) -> Result<Session, AuthError> {
        let username = normalize_account(username)?;
        let repository = AuthRepository::new(self.pool);
        if repository.user_by_username(&username).await?.is_some() {
            return Err(AuthError::UsernameTaken);
        }
        let mut id_bytes = [0_u8; 6];
        OsRng.fill_bytes(&mut id_bytes);
        let id = format!("u_{}", hex::encode(id_bytes));
        let input = NewUser {
            id: id.clone(),
            username,
            pass_hash: hash_password(password).await?,
            display_name,
            role: "member".into(),
        };
        let user = repository
            .create_user_with_invite(&input, invite)
            .await
            .map_err(|error| {
                if error.is_row_not_found() {
                    AuthError::InvalidInvite
                } else {
                    AuthError::Database(error)
                }
            })?;
        let token = self.create_session(&id).await?;
        repository.touch_last_login(&id).await?;
        Ok(Session {
            user: public_user(user),
            token,
        })
    }

    pub async fn create_owner(
        &self,
        username: &str,
        password: &str,
        display_name: Option<String>,
    ) -> Result<PublicUser, AuthError> {
        let repository = AuthRepository::new(self.pool);
        if repository.user_by_id(OWNER_USER_ID).await?.is_some() {
            return Err(AuthError::OwnerExists);
        }
        let user = repository
            .create_user(&NewUser {
                id: OWNER_USER_ID.into(),
                username: normalize_account(username)?,
                pass_hash: hash_password(password).await?,
                display_name,
                role: "owner".into(),
            })
            .await?;
        Ok(public_user(user))
    }

    pub async fn session_user(&self, token: Option<&str>) -> Result<Option<PublicUser>, AuthError> {
        let Some(token) = token else {
            return Ok(None);
        };
        let repository = AuthRepository::new(self.pool);
        let now = Utc::now();
        let token_hash = token_hash(token);
        let Some(session) = repository.live_session(&token_hash, now).await? else {
            return Ok(None);
        };
        if session.expires_at - now < Duration::days(SESSION_DAYS / 2) {
            repository
                .refresh_session(&token_hash, now + Duration::days(SESSION_DAYS))
                .await?;
        }
        Ok(repository
            .user_by_id(&session.user_id)
            .await?
            .map(public_user))
    }

    pub async fn destroy_session(&self, token: Option<&str>) -> Result<bool, AuthError> {
        match token {
            Some(token) => Ok(AuthRepository::new(self.pool)
                .delete_session(&token_hash(token))
                .await?),
            None => Ok(false),
        }
    }

    pub async fn create_invite(
        &self,
        owner: &PublicUser,
        note: Option<&str>,
    ) -> Result<String, AuthError> {
        if owner.role != UserRole::Owner {
            return Err(AuthError::InvalidCredentials);
        }
        let mut random = [0_u8; 4];
        OsRng.fill_bytes(&mut random);
        let code = format!("echo-{}", hex::encode(random));
        AuthRepository::new(self.pool)
            .create_invite(&code, note, Some(&owner.id))
            .await?;
        Ok(code)
    }

    async fn create_session(&self, user_id: &str) -> Result<String, AuthError> {
        let mut random = [0_u8; 32];
        OsRng.fill_bytes(&mut random);
        let token = URL_SAFE_NO_PAD.encode(random);
        let now = Utc::now();
        let repository = AuthRepository::new(self.pool);
        repository
            .insert_session(
                &token_hash(&token),
                user_id,
                now + Duration::days(SESSION_DAYS),
            )
            .await?;
        repository.prune_expired_sessions(now).await?;
        Ok(token)
    }
}

pub async fn hash_password(password: &str) -> Result<String, AuthError> {
    if password.chars().count() < 8 {
        return Err(AuthError::PasswordTooShort);
    }
    let password = password.to_string();
    tokio::task::spawn_blocking(move || {
        let mut salt = [0_u8; 16];
        OsRng.fill_bytes(&mut salt);
        hash_password_with_salt(&password, &salt)
    })
    .await
    .map_err(|_| AuthError::PasswordTask)?
}

pub async fn verify_password(password: &str, stored: &str) -> bool {
    let password = password.to_string();
    let stored = stored.to_string();
    tokio::task::spawn_blocking(move || verify_password_sync(&password, &stored))
        .await
        .unwrap_or(false)
}

fn scrypt_params() -> Result<Params, AuthError> {
    Params::new(14, 8, 1, 64).map_err(|_| AuthError::PasswordTask)
}

fn hash_password_with_salt(password: &str, salt: &[u8]) -> Result<String, AuthError> {
    let mut output = [0_u8; 64];
    scrypt(password.as_bytes(), salt, &scrypt_params()?, &mut output)
        .map_err(|_| AuthError::PasswordTask)?;
    Ok(format!("s1${}${}", hex::encode(salt), hex::encode(output)))
}

fn verify_password_sync(password: &str, stored: &str) -> bool {
    let mut parts = stored.split('$');
    let (Some("s1"), Some(salt), Some(expected), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let (Ok(salt), Ok(expected)) = (hex::decode(salt), hex::decode(expected)) else {
        return false;
    };
    let Ok(params) = scrypt_params() else {
        return false;
    };
    let mut actual = vec![0_u8; expected.len()];
    if scrypt(password.as_bytes(), &salt, &params, &mut actual).is_err() {
        return false;
    }
    actual.ct_eq(&expected).into()
}

fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn normalize_account(account: &str) -> Result<String, AuthError> {
    let account = account.trim().to_lowercase();
    let simple = (3..=24).contains(&account.len())
        && account
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'));
    let email = account.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty()
            && !local.chars().any(char::is_whitespace)
            && !domain.chars().any(char::is_whitespace)
            && domain
                .rsplit_once('.')
                .is_some_and(|(host, suffix)| !host.is_empty() && !suffix.is_empty())
    });
    if simple || email {
        Ok(account)
    } else {
        Err(AuthError::InvalidAccount)
    }
}

fn public_user(user: UserRow) -> PublicUser {
    PublicUser {
        id: user.id,
        username: user.username.clone(),
        display_name: Some(user.display_name.unwrap_or(user.username)),
        role: if user.role == "owner" {
            UserRole::Owner
        } else {
            UserRole::Member
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn password_hash_round_trip_and_wrong_password() {
        let hash = hash_password("correct horse").await.expect("hash");
        assert!(hash.starts_with("s1$"));
        assert!(verify_password("correct horse", &hash).await);
        assert!(!verify_password("wrong horse", &hash).await);
    }

    #[tokio::test]
    async fn malformed_hash_is_rejected_without_panic() {
        assert!(!verify_password("anything", "not-a-hash").await);
    }

    #[test]
    fn account_validation_matches_email_and_local_ids() {
        assert_eq!(normalize_account("A_B-C").expect("simple"), "a_b-c");
        assert_eq!(
            normalize_account("Name@Example.com").expect("email"),
            "name@example.com"
        );
        assert!(normalize_account("bad @example.com").is_err());
    }
}
