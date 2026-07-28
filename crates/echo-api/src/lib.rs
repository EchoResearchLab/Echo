//! Echo Research HTTP 边界（axum）。
//!
//!   * `GET  /health` / `/healthz`  —— 存活探针
//!   * `POST /api/ask` —— 研究入口：吃 { question, ticker, 行情/财报字段, 可选 draft_answer }，
//!                        经 `echo-domain` 跑**整条纯核**——意图路由 → 定点估值 → 决策面板 →
//!                        （给了草稿答案时）数字护栏——返回结构化结果。
//!
//! 取数（DB/行情/网页）由 `echo-db` 注入、模型网关与 SSE 流式作答随后接上；此处刻意让
//! 意图路由 + 估值 + 数字护栏这条**正确性关键路径先整体跑在 Rust 定点上**，因为它正是
//! 研究质量的病灶所在。单公司硬取值：面板与护栏只吃本请求这一家公司的财务事实，
//! 跨公司污染（"问苹果答腾讯"）在类型层面就发生不了。

use axum::extract::{DefaultBodyLimit, Extension, Path, Query, Request, State};
use axum::http::header::{COOKIE, ORIGIN, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::{
    Json, Router,
    routing::{get, post},
};
use echo_application::model_gateway::{
    AuditContext, ModelAnswerOptions, ModelStreamStart, OwnedAuditContext, ProviderConfig,
    model_answer, model_answer_stream,
};
use echo_application::{
    AuthError, AuthService, CompanyMemory, CompanyMemoryUpdate, CompanyResolvePorts,
    CompanyResolveService, DbCompanyHit, ExternalSymbolHit, FactRecovery, FactRecoveryRequest,
    GuardAuditRecord, LoadedFundamentals, PersistResearchSession, PriorTurn, ReportService,
    ResearchGap, ResearchPorts, ResearchService, ResolvedCompany, WatchRuleError, WatchRuleService,
    market_snapshot_from_rows, resolved_company_from_rows,
};
use echo_config::ApiConfig;
use echo_contracts::{
    AccountResponse, AccountSubscription, AskRequest, AskResponse, AuthInviteRequest,
    AuthInviteResponse, AuthLoginRequest, AuthLogoutResponse, AuthMeResponse, AuthRegisterRequest,
    AuthUserResponse, ChangedCountResponse, CompanyProfileDetail, CompanyProfileResponse,
    CompanyProfileSummary, CompanyProfileUpsertRequest, CompanyProfilesListResponse,
    CompanyResolveItem, CompanyResolveQuery, CompanyResolveResponse, CompanySearchItem,
    CompanySearchQuery, CompanySearchResponse, CompanyVerifyQuery, CompanyVerifyResponse,
    CompanyVerifySuggestion, CompareRequest, CompareResponse, DeskResponse, DeskTicker,
    ErrorResponse, HealthResponse, ListQuery, MutationResponse, Notification,
    NotificationReadRequest, NotificationsListResponse, PortfolioListResponse, PortfolioPosition,
    PortfolioUpsertRequest, PreferencesResponse, PreferencesUpdateRequest, PublicUser,
    ReportGenerateResponse, ResearchSessionDetail, ResearchSessionResponse, ResearchSessionSummary,
    ResearchSessionsResponse, ResearchStreamEvent, TickerQuery, UnreadResponse, UserPreferences,
    UserRole, WatchEntry, WatchListResponse, WatchMutationRequest, WatchRule,
    WatchRuleCreateRequest, WatchRuleDeleteRequest, WatchRulesListResponse,
};
use echo_data::{
    CalendarService, EvidenceService, FilingsService, FmpSearchService, FundamentalsRow,
    FundamentalsService, HistoricalValuationService, HkAnnualReportService, Market,
    NormalizedHkFinancials, PeerService, QuoteService, SecCompanyFactsService, SecFundamentals,
    detect_market, normalize_ticker, pct_change, pct_of,
};
use echo_db::{
    AuthRepository, CompanyProfileRepository, CompanyProfileUpsert, CompanyRepository,
    FactGuardAuditEntry, FactGuardAuditRepository, FactGuardHardDetail, HkFinancialsRepository,
    HkFinancialsRow, MarketRepository, NotificationsRepository, Pool, PortfolioRepository,
    PortfolioUpsert, PreferencesPatch, PreferencesRepository, RateLimitRepository,
    ResearchSessionRepository, SaveResearchSession, UserPreferencesRow, WatchlistRepository,
};
use echo_domain::{
    EarningsCalendar, Evidence, Filing, Financials, HistoricalValuation, MarketSnapshot,
    MultipleType, PeerAnchor, company_identity_key, has_compare_cue, match_company_mentions,
};
use futures_util::Stream;
use std::convert::Infallible;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tower_http::trace::{DefaultMakeSpan, TraceLayer};
use tracing::{error, info, warn};

const COOKIE_NAME: &str = "echo_session";
const SESSION_MAX_AGE_SECONDS: i64 = 30 * 86_400;
/// 请求体上限：研究请求体是结构化数字 + 一段问题/草稿文本，512KiB 留足余量，拦掉异常大包。
const MAX_JSON_BODY_BYTES: usize = 512 * 1024;
const ASK_RATE_LIMIT_WINDOW_SECONDS: i64 = 60;

/// 共享状态：可选的数据库连接池。未配 `DATABASE_URL` 时为 `None`——`/api/ask` 只吃请求体里带的
/// 数字（纯核路径，可离库端到端验证）；配了库则在缺行情时从 `echo-db` 兜底最新快照。
/// FMP fundamentals/search 不依赖数据库，有配置即可注入研究端口。
#[derive(Clone)]
pub struct AppState {
    pool: Option<Pool>,
    quotes: Option<QuoteService>,
    fundamentals: Option<FundamentalsService>,
    calendar: Option<CalendarService>,
    historical_valuation: Option<HistoricalValuationService>,
    sec_company_facts: Option<SecCompanyFactsService>,
    hk_annual_reports: Option<HkAnnualReportService>,
    peers: Option<PeerService>,
    filings: Option<FilingsService>,
    evidence: Option<EvidenceService>,
    fmp_search: Option<FmpSearchService>,
    auth_disabled: bool,
    auth_disabled_user_id: String,
    secure_cookie: bool,
    model_provider: Option<ProviderConfig>,
    allowed_origins: Vec<String>,
    ask_rate_limit_per_minute: u32,
}

impl AppState {
    #[must_use]
    pub fn without_database() -> Self {
        Self {
            pool: None,
            quotes: None,
            fundamentals: None,
            calendar: None,
            historical_valuation: None,
            sec_company_facts: None,
            hk_annual_reports: None,
            peers: None,
            filings: None,
            evidence: None,
            fmp_search: None,
            auth_disabled: true,
            auth_disabled_user_id: "local".into(),
            secure_cookie: false,
            model_provider: None,
            allowed_origins: vec!["http://localhost:5191".into()],
            ask_rate_limit_per_minute: 20,
        }
    }
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn database_required() -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "此功能需要 PostgreSQL")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                message: self.message,
            }),
        )
            .into_response()
    }
}

fn map_auth_error(error: AuthError) -> ApiError {
    let status = match error {
        AuthError::InvalidCredentials => StatusCode::UNAUTHORIZED,
        AuthError::PasswordTooShort
        | AuthError::InvalidAccount
        | AuthError::UsernameTaken
        | AuthError::InvalidInvite
        | AuthError::OwnerExists => StatusCode::BAD_REQUEST,
        AuthError::Database(_) | AuthError::PasswordTask => StatusCode::INTERNAL_SERVER_ERROR,
    };
    if status == StatusCode::INTERNAL_SERVER_ERROR {
        error!(error = %error, "认证操作失败");
        ApiError::new(status, "认证服务暂时不可用")
    } else {
        ApiError::new(status, error.to_string())
    }
}

fn map_db_error(error: echo_db::DbError) -> ApiError {
    error!(error = %error, "数据库操作失败");
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "数据库操作失败")
}

fn map_watch_rule_error(error: WatchRuleError) -> ApiError {
    match error {
        WatchRuleError::Db(error) => map_db_error(error),
        other => ApiError::new(StatusCode::BAD_REQUEST, other.to_string()),
    }
}

fn require_pool(state: &AppState) -> Result<&Pool, ApiError> {
    state.pool.as_ref().ok_or_else(ApiError::database_required)
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse::ok())
}

/// 就绪探针：配了 `DATABASE_URL` 就必须能连上（`SELECT 1`），否则 503——流量不该被路由到
/// 一个数据库掉线的副本。未配库属有意的纯核部署，视为就绪（与 `AppState::without_database`
/// 的设计一致，不是降级）。
async fn ready(State(state): State<AppState>) -> Response {
    let Some(pool) = state.pool.as_ref() else {
        return Json(HealthResponse::ok()).into_response();
    };
    match echo_db::ping(pool).await {
        Ok(()) => Json(HealthResponse::ok()).into_response(),
        Err(error) => {
            error!(error = %error, "readiness 探针失败：数据库不可达");
            ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "数据库不可达").into_response()
        }
    }
}

/// Origin 校验（CSRF 防护）：状态变更请求带 `Origin` 头时必须在白名单内；非浏览器客户端
/// 通常不带该头，放行——拦的是"浏览器从别的站点悄悄提交这个会话的 Cookie"这类跨站请求。
async fn enforce_origin(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) {
        if let Some(origin) = request
            .headers()
            .get(ORIGIN)
            .and_then(|value| value.to_str().ok())
        {
            if !state
                .allowed_origins
                .iter()
                .any(|allowed| allowed == origin)
            {
                warn!(origin, "Origin 校验拒绝跨站请求");
                return ApiError::new(StatusCode::FORBIDDEN, "非法请求来源").into_response();
            }
        }
    }
    next.run(request).await
}

/// 研究端点限流：按用户 + 60 秒滑动窗口共享 `rate_limit_buckets`，挡掉对昂贵模型调用的
/// 高频重放。仅在配库时生效；限流查询自身出错按放行处理（限流故障不该拖垮研究主链）。
async fn rate_limit_ask(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(pool) = state.pool.as_ref() {
        let key = format!("ask:{}", user.id);
        match RateLimitRepository::new(pool)
            .try_consume(
                &key,
                state.ask_rate_limit_per_minute as i32,
                ASK_RATE_LIMIT_WINDOW_SECONDS,
            )
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                return ApiError::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    "研究请求过于频繁，请稍后再试",
                )
                .into_response();
            }
            Err(error) => {
                error!(error = %error, "限流检查失败，本次放行");
            }
        }
    }
    next.run(request).await
}

fn request_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|part| {
                let (name, value) = part.trim().split_once('=')?;
                (name == COOKIE_NAME).then_some(value)
            })
        })
}

fn session_cookie(token: &str, clear: bool, secure: bool) -> Result<HeaderValue, ApiError> {
    let secure = if secure { "; Secure" } else { "" };
    let value = if clear {
        format!("{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure}")
    } else {
        format!(
            "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_MAX_AGE_SECONDS}{secure}"
        )
    };
    HeaderValue::from_str(&value)
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "会话 Cookie 生成失败"))
}

fn local_public_user(id: &str) -> PublicUser {
    PublicUser {
        id: id.to_string(),
        username: "local".into(),
        display_name: Some("本机用户".into()),
        role: UserRole::Owner,
    }
}

async fn resolve_request_user(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<PublicUser>, ApiError> {
    let Some(pool) = &state.pool else {
        return Ok(Some(local_public_user(&state.auth_disabled_user_id)));
    };
    let auth = AuthService::new(pool);
    if state.auth_disabled {
        return auth
            .local_owner(&state.auth_disabled_user_id)
            .await
            .map(Some)
            .map_err(map_auth_error);
    }
    auth.session_user(request_token(headers))
        .await
        .map_err(map_auth_error)
}

async fn require_auth(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    match resolve_request_user(&state, request.headers()).await {
        Ok(Some(user)) => {
            request.extensions_mut().insert(user);
            next.run(request).await
        }
        Ok(None) => ApiError::new(StatusCode::UNAUTHORIZED, "请先登录").into_response(),
        Err(error) => error.into_response(),
    }
}

async fn auth_login(
    State(state): State<AppState>,
    Json(input): Json<AuthLoginRequest>,
) -> Result<Response, ApiError> {
    let pool = state
        .pool
        .as_ref()
        .ok_or_else(ApiError::database_required)?;
    let session = AuthService::new(pool)
        .login(&input.username, &input.password)
        .await
        .map_err(map_auth_error)?;
    let mut response = Json(AuthUserResponse { user: session.user }).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        session_cookie(&session.token, false, state.secure_cookie)?,
    );
    Ok(response)
}

async fn auth_register(
    State(state): State<AppState>,
    Json(input): Json<AuthRegisterRequest>,
) -> Result<Response, ApiError> {
    let pool = state
        .pool
        .as_ref()
        .ok_or_else(ApiError::database_required)?;
    let session = AuthService::new(pool)
        .register(
            &input.invite,
            &input.username,
            &input.password,
            input.display_name,
        )
        .await
        .map_err(map_auth_error)?;
    let mut response = Json(AuthUserResponse { user: session.user }).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        session_cookie(&session.token, false, state.secure_cookie)?,
    );
    Ok(response)
}

async fn auth_logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(pool) = &state.pool {
        AuthService::new(pool)
            .destroy_session(request_token(&headers))
            .await
            .map_err(map_auth_error)?;
    }
    let mut response = Json(AuthLogoutResponse { logged_out: true }).into_response();
    response
        .headers_mut()
        .insert(SET_COOKIE, session_cookie("", true, state.secure_cookie)?);
    Ok(response)
}

async fn auth_me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AuthMeResponse>, ApiError> {
    let user = resolve_request_user(&state, &headers).await?;
    let multi_user = state.pool.is_some() && !state.auth_disabled;
    Ok(Json(AuthMeResponse { user, multi_user }))
}

async fn account_get(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
) -> Result<Json<AccountResponse>, ApiError> {
    let subscription = AuthRepository::new(require_pool(&state)?)
        .subscription_for_user(&user.id)
        .await
        .map_err(map_db_error)?
        .map(|row| AccountSubscription {
            plan_id: row.plan_id,
            plan_name: row.plan_name,
            tier: row.tier,
            status: row.status,
            current_period_end: row.current_period_end.to_rfc3339(),
            max_daily_calls: row.max_daily_calls,
            features: row.features,
        });
    Ok(Json(AccountResponse { user, subscription }))
}

async fn auth_invite(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
    Json(input): Json<AuthInviteRequest>,
) -> Result<Json<AuthInviteResponse>, ApiError> {
    if user.role != UserRole::Owner {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "只有 owner 能生成邀请码",
        ));
    }
    let pool = state
        .pool
        .as_ref()
        .ok_or_else(ApiError::database_required)?;
    let code = AuthService::new(pool)
        .create_invite(&user, input.note.as_deref())
        .await
        .map_err(map_auth_error)?;
    Ok(Json(AuthInviteResponse { code }))
}

async fn companies_search(
    State(state): State<AppState>,
    Query(query): Query<CompanySearchQuery>,
) -> Result<Json<CompanySearchResponse>, ApiError> {
    let rows = CompanyRepository::new(require_pool(&state)?)
        .search(&query.q, query.limit.unwrap_or(20))
        .await
        .map_err(map_db_error)?;
    Ok(Json(CompanySearchResponse {
        companies: rows
            .into_iter()
            .map(|row| CompanySearchItem {
                ticker: row.ticker,
                name_zh: row.name_zh,
                name_en: row.name_en,
                sector: row.sector,
                industry: row.industry,
                has_portrait: row.has_portrait,
            })
            .collect(),
    }))
}

struct ApiCompanyResolvePorts {
    state: AppState,
}

impl CompanyResolvePorts for ApiCompanyResolvePorts {
    async fn db_by_ticker(&self, ticker: &str) -> Option<DbCompanyHit> {
        let pool = self.state.pool.as_ref()?;
        let row = CompanyRepository::new(pool)
            .by_ticker(ticker)
            .await
            .ok()??;
        Some(DbCompanyHit {
            ticker: row.ticker,
            name_zh: row.name_zh,
            name_en: row.name_en,
            industry: row.industry,
        })
    }

    async fn db_search(&self, query: &str, limit: i64) -> Vec<DbCompanyHit> {
        let Some(pool) = self.state.pool.as_ref() else {
            return Vec::new();
        };
        CompanyRepository::new(pool)
            .search(query, limit)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|row| DbCompanyHit {
                ticker: row.ticker,
                name_zh: row.name_zh,
                name_en: row.name_en,
                industry: row.industry,
            })
            .collect()
    }

    async fn fmp_exact_us(&self, ticker: &str) -> Option<ExternalSymbolHit> {
        let search = self.state.fmp_search.as_ref()?;
        let hit = search.exact_us_hit(ticker).await?;
        Some(ExternalSymbolHit {
            symbol: hit.symbol,
            name: hit.name,
            exchange: hit.exchange,
        })
    }

    async fn fmp_search_name(&self, name: &str) -> Vec<ExternalSymbolHit> {
        let Some(search) = self.state.fmp_search.as_ref() else {
            return Vec::new();
        };
        search
            .search_name(name)
            .await
            .into_iter()
            .map(|hit| ExternalSymbolHit {
                symbol: hit.symbol,
                name: hit.name,
                exchange: hit.exchange,
            })
            .collect()
    }

    async fn quote_alive(&self, ticker: &str) -> bool {
        let Some(quotes) = self.state.quotes.as_ref() else {
            return false;
        };
        matches!(quotes.fetch_live(ticker).await, Ok(routed) if routed.quote.price.is_some())
    }
}

async fn companies_resolve(
    State(state): State<AppState>,
    Query(query): Query<CompanyResolveQuery>,
) -> Json<CompanyResolveResponse> {
    let ports = ApiCompanyResolvePorts { state };
    let result = CompanyResolveService::resolve_query(&ports, &query.q).await;
    Json(CompanyResolveResponse {
        company: result.company.map(|company| CompanyResolveItem {
            ticker: company.ticker,
            name_zh: company.name_zh,
            name_en: (!company.name_en.is_empty()).then_some(company.name_en),
            industry: (!company.industry.is_empty()).then_some(company.industry),
        }),
        reason: result.reason,
    })
}

async fn companies_verify(
    State(state): State<AppState>,
    Query(query): Query<CompanyVerifyQuery>,
) -> Json<CompanyVerifyResponse> {
    let ports = ApiCompanyResolvePorts { state };
    let result = CompanyResolveService::verify_ticker(&ports, &query.ticker).await;
    let status = match result.status {
        echo_application::VerifyStatus::Verified => "verified",
        echo_application::VerifyStatus::NotFound => "not_found",
    };
    Json(CompanyVerifyResponse {
        status: status.into(),
        name: result.name.filter(|value| !value.is_empty()),
        suggestions: (!result.suggestions.is_empty()).then(|| {
            result
                .suggestions
                .into_iter()
                .map(|item| CompanyVerifySuggestion {
                    ticker: item.ticker,
                    name: item.name,
                })
                .collect()
        }),
    })
}

/// 研究入口：验证主体（必要时从问题解析）并在有库时 ensure 建档。
async fn prepare_research_request(state: &AppState, mut req: AskRequest) -> AskRequest {
    let ports = ApiCompanyResolvePorts {
        state: state.clone(),
    };
    let ticker = {
        let trimmed = req.ticker.trim();
        (!trimmed.is_empty()).then_some(trimmed)
    };
    let Some(listing) = CompanyResolveService::resolve_research_company(
        &ports,
        ticker,
        req.name_zh.as_deref(),
        &req.question,
    )
    .await
    else {
        return req;
    };
    if let Some(pool) = &state.pool {
        if let Err(error) = CompanyRepository::new(pool)
            .ensure(
                &listing.ticker,
                Some(listing.name_zh.as_str()),
                (!listing.name_en.is_empty()).then_some(listing.name_en.as_str()),
                None,
                (!listing.industry.is_empty()).then_some(listing.industry.as_str()),
            )
            .await
        {
            warn!(
                ticker = %listing.ticker,
                error = %error,
                "验证通过后建档失败，本轮仍继续研究"
            );
        }
    }
    req.ticker = listing.ticker;
    if req.name_zh.is_none() {
        req.name_zh = Some(listing.name_zh);
    }
    req
}

async fn watch_list(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
) -> Result<Json<WatchListResponse>, ApiError> {
    let rows = WatchlistRepository::new(require_pool(&state)?)
        .list(&user.id)
        .await
        .map_err(map_db_error)?;
    Ok(Json(WatchListResponse {
        entries: rows
            .into_iter()
            .map(|row| WatchEntry {
                ticker: row.ticker,
                company_name: row.company_name,
                mode: row.mode,
                created_at: row.created_at.to_rfc3339(),
            })
            .collect(),
    }))
}

async fn watch_track(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
    Json(input): Json<WatchMutationRequest>,
) -> Result<Json<MutationResponse>, ApiError> {
    let changed = WatchlistRepository::new(require_pool(&state)?)
        .set(
            &user.id,
            &input.ticker,
            input.company_name.as_deref(),
            "add",
        )
        .await
        .map_err(map_db_error)?;
    Ok(Json(MutationResponse { changed }))
}

async fn watch_untrack(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
    Json(input): Json<WatchMutationRequest>,
) -> Result<Json<MutationResponse>, ApiError> {
    let changed = WatchlistRepository::new(require_pool(&state)?)
        .set(&user.id, &input.ticker, None, "hide")
        .await
        .map_err(map_db_error)?;
    Ok(Json(MutationResponse { changed }))
}

fn watch_rule_view(row: echo_db::WatchRuleDetailRow) -> WatchRule {
    WatchRule {
        id: row.id,
        ticker: row.ticker,
        kind: row.kind,
        threshold: row.threshold,
        metric: row.metric,
        label: row.label,
        active: row.active,
        created_at: row.created_at.to_rfc3339(),
        last_triggered_at: row.last_triggered_at.map(|value| value.to_rfc3339()),
    }
}

async fn watch_rules_list(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
) -> Result<Json<WatchRulesListResponse>, ApiError> {
    let rules = WatchRuleService::new(require_pool(&state)?)
        .list(&user.id)
        .await
        .map_err(map_watch_rule_error)?
        .into_iter()
        .map(watch_rule_view)
        .collect();
    Ok(Json(WatchRulesListResponse { rules }))
}

async fn watch_rules_create(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
    Json(input): Json<WatchRuleCreateRequest>,
) -> Result<Json<WatchRule>, ApiError> {
    let row = WatchRuleService::new(require_pool(&state)?)
        .create(
            &user.id,
            &input.ticker,
            &input.kind,
            input.threshold,
            input.metric.as_deref(),
            input.label.as_deref(),
        )
        .await
        .map_err(map_watch_rule_error)?;
    Ok(Json(watch_rule_view(row)))
}

async fn watch_rules_delete(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
    Query(input): Query<WatchRuleDeleteRequest>,
) -> Result<Json<MutationResponse>, ApiError> {
    let changed = WatchRuleService::new(require_pool(&state)?)
        .delete(&user.id, input.id)
        .await
        .map_err(map_watch_rule_error)?;
    Ok(Json(MutationResponse { changed }))
}

/// 自选台面聚合：已跟踪 ticker（关注列表 + 持仓 + 有规则但未在前两者出现的 ticker）
/// 各自的最新行情、挂载的监控规则，以及近期触发通知——全部只读聚合已有仓储，不新增写路径。
async fn watch_desk(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
) -> Result<Json<DeskResponse>, ApiError> {
    let pool = require_pool(&state)?;
    let watchlist = WatchlistRepository::new(pool)
        .list(&user.id)
        .await
        .map_err(map_db_error)?;
    let positions = PortfolioRepository::new(pool)
        .list(&user.id)
        .await
        .map_err(map_db_error)?;
    let rules = WatchRuleService::new(pool)
        .list(&user.id)
        .await
        .map_err(map_watch_rule_error)?;

    let mut names: std::collections::BTreeMap<String, Option<String>> =
        std::collections::BTreeMap::new();
    for entry in watchlist.into_iter().filter(|entry| entry.mode == "add") {
        names.insert(entry.ticker, entry.company_name);
    }
    for position in &positions {
        names
            .entry(position.ticker.clone())
            .or_insert_with(|| position.company_name.clone());
    }
    let mut rules_by_ticker: std::collections::BTreeMap<String, Vec<WatchRule>> =
        std::collections::BTreeMap::new();
    for rule in rules {
        names.entry(rule.ticker.clone()).or_insert(None);
        rules_by_ticker
            .entry(rule.ticker.clone())
            .or_default()
            .push(watch_rule_view(rule));
    }

    let market = MarketRepository::new(pool);
    let mut tickers = Vec::with_capacity(names.len());
    for (ticker, company_name) in names {
        let snapshot = market
            .latest_snapshot(&ticker)
            .await
            .map_err(map_db_error)?;
        tickers.push(DeskTicker {
            ticker: ticker.clone(),
            company_name,
            price: snapshot.as_ref().and_then(|row| row.price),
            change_percent: snapshot.as_ref().and_then(|row| row.change_percent),
            rules: rules_by_ticker.remove(&ticker).unwrap_or_default(),
        });
    }

    let recent_triggers = NotificationsRepository::new(pool)
        .list(&user.id, 10)
        .await
        .map_err(map_db_error)?
        .into_iter()
        .filter(|row| {
            matches!(
                row.kind.as_str(),
                "falsify_alert" | "position_alert" | "earnings_review"
            )
        })
        .map(notification_view)
        .collect();

    Ok(Json(DeskResponse {
        tickers,
        recent_triggers,
    }))
}

fn portfolio_position(row: echo_db::PortfolioPositionRow) -> PortfolioPosition {
    PortfolioPosition {
        company_name: row.company_name.unwrap_or_else(|| row.ticker.clone()),
        ticker: row.ticker,
        shares: row.shares,
        avg_cost: row.avg_cost,
        stop_loss: row.stop_loss,
        take_profit: row.take_profit,
        note: row.note.unwrap_or_default(),
        updated_at: row.updated_at.to_rfc3339(),
    }
}

async fn portfolio_list(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
) -> Result<Json<PortfolioListResponse>, ApiError> {
    let positions = PortfolioRepository::new(require_pool(&state)?)
        .list(&user.id)
        .await
        .map_err(map_db_error)?
        .into_iter()
        .map(portfolio_position)
        .collect();
    Ok(Json(PortfolioListResponse { positions }))
}

async fn portfolio_upsert(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
    Json(input): Json<PortfolioUpsertRequest>,
) -> Result<Json<PortfolioPosition>, ApiError> {
    if input.shares <= echo_contracts::Decimal::ZERO
        || input.avg_cost < echo_contracts::Decimal::ZERO
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "持有股数必须大于 0，平均成本不得为负",
        ));
    }
    let row = PortfolioRepository::new(require_pool(&state)?)
        .upsert(
            &user.id,
            &input.ticker,
            &PortfolioUpsert {
                company_name: input.company_name,
                shares: Some(input.shares),
                avg_cost: Some(input.avg_cost),
                stop_loss: input.stop_loss,
                take_profit: input.take_profit,
                note: Some(input.note),
            },
        )
        .await
        .map_err(map_db_error)?;
    Ok(Json(portfolio_position(row)))
}

async fn portfolio_delete(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
    Query(query): Query<TickerQuery>,
) -> Result<Json<MutationResponse>, ApiError> {
    let changed = PortfolioRepository::new(require_pool(&state)?)
        .delete(&user.id, &query.ticker)
        .await
        .map_err(map_db_error)?;
    Ok(Json(MutationResponse { changed }))
}

fn company_profile_summary(row: echo_db::CompanyProfileSummaryRow) -> CompanyProfileSummary {
    CompanyProfileSummary {
        ticker: row.ticker,
        company_name: row.company_name,
        research_status: row.research_status,
        confidence: row.confidence,
        turn_count: row.turn_count,
        updated_at: row.updated_at.to_rfc3339(),
    }
}

fn company_profile_detail(row: echo_db::CompanyProfileRow) -> CompanyProfileDetail {
    CompanyProfileDetail {
        ticker: row.ticker,
        company_name: row.company_name,
        thesis: row.thesis,
        research_status: row.research_status,
        confidence: row.confidence,
        bull: row.bull.unwrap_or_default(),
        bear: row.bear.unwrap_or_default(),
        monitors: row.monitors.unwrap_or_default(),
        falsifiers: row.falsifiers.unwrap_or_default(),
        valuation_method: row.valuation_method,
        valuation_bear: row.valuation_bear,
        valuation_base: row.valuation_base,
        valuation_bull: row.valuation_bull,
        valuation_current_price: row.valuation_current_price,
        profile_md: row.profile_md,
        turn_count: row.turn_count,
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
    }
}

async fn profiles_list(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
    Query(query): Query<ListQuery>,
) -> Result<Json<CompanyProfilesListResponse>, ApiError> {
    let profiles = CompanyProfileRepository::new(require_pool(&state)?)
        .list(&user.id, query.limit.unwrap_or(50))
        .await
        .map_err(map_db_error)?
        .into_iter()
        .map(company_profile_summary)
        .collect();
    Ok(Json(CompanyProfilesListResponse { profiles }))
}

async fn profile_get(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
    Path(ticker): Path<String>,
) -> Result<Json<CompanyProfileResponse>, ApiError> {
    let profile = CompanyProfileRepository::new(require_pool(&state)?)
        .get(&user.id, &ticker)
        .await
        .map_err(map_db_error)?
        .map(company_profile_detail);
    Ok(Json(CompanyProfileResponse { profile }))
}

async fn profile_upsert(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
    Path(ticker): Path<String>,
    Json(input): Json<CompanyProfileUpsertRequest>,
) -> Result<Json<CompanyProfileDetail>, ApiError> {
    let row = CompanyProfileRepository::new(require_pool(&state)?)
        .upsert(
            &user.id,
            &ticker,
            &CompanyProfileUpsert {
                company_name: input.company_name,
                thesis: input.thesis,
                research_status: input.research_status,
                confidence: input.confidence,
                bull: input.bull,
                bear: input.bear,
                monitors: input.monitors,
                falsifiers: input.falsifiers,
                valuation_method: input.valuation_method,
                valuation_bear: input.valuation_bear,
                valuation_base: input.valuation_base,
                valuation_bull: input.valuation_bull,
                valuation_current_price: input.valuation_current_price,
                profile_md: input.profile_md,
            },
        )
        .await
        .map_err(map_db_error)?;
    Ok(Json(company_profile_detail(row)))
}

async fn profile_delete(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
    Path(ticker): Path<String>,
) -> Result<Json<MutationResponse>, ApiError> {
    let changed = CompanyProfileRepository::new(require_pool(&state)?)
        .delete(&user.id, &ticker)
        .await
        .map_err(map_db_error)?;
    Ok(Json(MutationResponse { changed }))
}

fn preferences(row: UserPreferencesRow) -> UserPreferences {
    UserPreferences {
        onboarding_completed: row.onboarding_completed,
        notify_digest: row.notify_digest,
        notify_positions: row.notify_positions,
        notify_falsify: row.notify_falsify,
        notify_review: row.notify_review,
        notify_earnings: row.notify_earnings,
        quiet_hours_start: row.quiet_hours_start,
        quiet_hours_end: row.quiet_hours_end,
    }
}

async fn preferences_get(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
) -> Result<Json<PreferencesResponse>, ApiError> {
    let row = PreferencesRepository::new(require_pool(&state)?)
        .get(&user.id)
        .await
        .map_err(map_db_error)?;
    Ok(Json(PreferencesResponse {
        preferences: preferences(row),
    }))
}

fn valid_hhmm(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 5
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2] == b':'
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
        && (bytes[0] - b'0') * 10 + (bytes[1] - b'0') < 24
        && (bytes[3] - b'0') * 10 + (bytes[4] - b'0') < 60
}

async fn preferences_update(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
    Json(input): Json<PreferencesUpdateRequest>,
) -> Result<Json<PreferencesResponse>, ApiError> {
    for value in [
        input.quiet_hours_start.as_deref(),
        input.quiet_hours_end.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if !value.is_empty() && !valid_hhmm(value) {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "免打扰时间必须是 HH:MM",
            ));
        }
    }
    let row = PreferencesRepository::new(require_pool(&state)?)
        .update(
            &user.id,
            &PreferencesPatch {
                onboarding_completed: input.onboarding_completed,
                notify_digest: input.notify_digest,
                notify_positions: input.notify_positions,
                notify_falsify: input.notify_falsify,
                notify_review: input.notify_review,
                notify_earnings: input.notify_earnings,
                quiet_hours_start: input.quiet_hours_start,
                quiet_hours_end: input.quiet_hours_end,
            },
        )
        .await
        .map_err(map_db_error)?;
    Ok(Json(PreferencesResponse {
        preferences: preferences(row),
    }))
}

fn notification_view(row: echo_db::NotificationRow) -> Notification {
    Notification {
        id: row.id,
        kind: row.kind,
        title: row.title,
        body: row.body.unwrap_or_default(),
        ticker: row.ticker,
        payload: row.payload,
        created_at: row.created_at.to_rfc3339(),
        read_at: row.read_at.map(|date| date.to_rfc3339()),
    }
}

async fn notifications_list(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
    Query(query): Query<ListQuery>,
) -> Result<Json<NotificationsListResponse>, ApiError> {
    let notifications = NotificationsRepository::new(require_pool(&state)?)
        .list(&user.id, query.limit.unwrap_or(20))
        .await
        .map_err(map_db_error)?
        .into_iter()
        .map(notification_view)
        .collect();
    Ok(Json(NotificationsListResponse { notifications }))
}

async fn notifications_unread(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
) -> Result<Json<UnreadResponse>, ApiError> {
    let unread = NotificationsRepository::new(require_pool(&state)?)
        .unread_count(&user.id)
        .await
        .map_err(map_db_error)?;
    Ok(Json(UnreadResponse { unread }))
}

async fn notifications_read(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
    Json(input): Json<NotificationReadRequest>,
) -> Result<Json<ChangedCountResponse>, ApiError> {
    let changed = NotificationsRepository::new(require_pool(&state)?)
        .mark_read(&user.id, input.id)
        .await
        .map_err(map_db_error)?;
    Ok(Json(ChangedCountResponse { changed }))
}

fn research_summary(row: echo_db::ResearchSessionSummaryRow) -> ResearchSessionSummary {
    ResearchSessionSummary {
        conversation_id: row.conversation_id.unwrap_or_else(|| row.id.clone()),
        title: row
            .title
            .or_else(|| row.question.clone())
            .unwrap_or_default(),
        question: row.question.unwrap_or_default(),
        id: row.id,
        ticker: row.ticker,
        status: row.status,
        rating: row.rating,
        confidence: row.confidence,
        turn_count: row.turn_count.unwrap_or(0),
        company_name: row.company_name,
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
    }
}

fn research_detail(row: echo_db::ResearchSessionRow) -> ResearchSessionDetail {
    ResearchSessionDetail {
        conversation_id: row.conversation_id.unwrap_or_else(|| row.id.clone()),
        title: row
            .title
            .or_else(|| row.question.clone())
            .unwrap_or_default(),
        question: row.question.unwrap_or_default(),
        id: row.id,
        ticker: row.ticker,
        status: row.status,
        report_markdown: row.report_markdown,
        rating: row.rating,
        confidence: row.confidence,
        decision_panel: row.decision_panel,
        full_research: row.full_research,
        data_sources: row.data_sources,
        thread: row.thread_json,
        turn_count: row.turn_count.unwrap_or(0),
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
    }
}

async fn research_sessions_list(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ResearchSessionsResponse>, ApiError> {
    let sessions: Vec<_> = ResearchSessionRepository::new(require_pool(&state)?)
        .list(&user.id, query.ticker.as_deref(), query.limit.unwrap_or(20))
        .await
        .map_err(map_db_error)?
        .into_iter()
        .map(research_summary)
        .collect();
    let count = sessions.len();
    Ok(Json(ResearchSessionsResponse { sessions, count }))
}

async fn research_session_get(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
    Path(id): Path<String>,
) -> Result<Json<ResearchSessionResponse>, ApiError> {
    let session = ResearchSessionRepository::new(require_pool(&state)?)
        .get(&user.id, &id)
        .await
        .map_err(map_db_error)?
        .map(research_detail);
    Ok(Json(ResearchSessionResponse { session }))
}

async fn research_session_delete(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
    Path(id): Path<String>,
) -> Result<Json<MutationResponse>, ApiError> {
    let changed = ResearchSessionRepository::new(require_pool(&state)?)
        .delete(&user.id, &id)
        .await
        .map_err(map_db_error)?;
    Ok(Json(MutationResponse { changed }))
}

async fn research_sessions_clear(
    State(state): State<AppState>,
    Extension(user): Extension<PublicUser>,
) -> Result<Json<ChangedCountResponse>, ApiError> {
    let changed = ResearchSessionRepository::new(require_pool(&state)?)
        .clear(&user.id)
        .await
        .map_err(map_db_error)?;
    Ok(Json(ChangedCountResponse { changed }))
}

/// HTTP 边界上的研究端口适配：DB 补数、行情刷新、FMP 财务、模型生成、会话落库。
/// 持有 `AppState` 所有权，以便流式用例在 SSE 生命周期内驱动。
struct ApiResearchPorts {
    state: AppState,
}

fn financials_from_fmp_row(row: &FundamentalsRow) -> Financials {
    Financials {
        provider_ok: true,
        currency: row.currency.clone(),
        revenue: row.revenue,
        gross_profit: row.gross_profit,
        operating_income: row.operating_income,
        net_income: row.net_income,
        operating_cash_flow: row.operating_cash_flow,
        cash_and_equivalents: row.cash_and_equivalents,
        net_cash: row.net_cash,
        // EPS 优先用 TTM（`netIncomePerShareTTM`）并标为已年化，估值 PE 法才成立；供应商这轮
        // 没给 TTM 时退回单季并标未年化——宁可不给估值区间，也不拿单季 EPS 反推出四倍虚高的
        // 目标价。研究语境下"每股收益"本就该是 TTM 口径，护栏登记的也是这个值。
        eps: row.eps_ttm.or(row.eps),
        eps_annualized: row.eps_ttm.is_none().then_some(false),
        pe: row.pe_ttm,
        free_cash_flow: row.free_cash_flow_ttm,
        shares_outstanding: row.shares_outstanding,
        total_debt: row.total_debt,
        revenue_growth: pct_change(row.revenue, row.revenue_prior),
        gross_margin: pct_of(row.gross_profit, row.revenue),
        operating_margin: pct_of(row.operating_income, row.revenue),
        net_margin: pct_of(row.net_income, row.revenue),
        profit_growth: pct_change(row.net_income, row.net_income_prior),
        period: row.period_label.clone().or_else(|| row.period_end.clone()),
        ..Default::default()
    }
}

/// 港股一手财报行 → `Financials`。历史行没有单位归一化证明时仍只取同一行内的比率与 EPS；
/// 受控 ingest 写入的 `amounts_normalized=true` 行才允许绝对营收/利润/现金流进入研究事实。
/// `source_unit_scale` 只用于追溯，库内金额已经是绝对值，读侧绝不二次乘倍率。
fn financials_from_hk_row(row: &HkFinancialsRow) -> Financials {
    let trusted = row.amounts_normalized;
    Financials {
        provider_ok: true,
        currency: row.currency.clone(),
        revenue: trusted.then_some(row.revenue).flatten(),
        gross_profit: trusted.then_some(row.gross_profit).flatten(),
        operating_income: trusted.then_some(row.operating_income).flatten(),
        net_income: trusted.then_some(row.net_income).flatten(),
        operating_cash_flow: trusted.then_some(row.operating_cash_flow).flatten(),
        cash_and_equivalents: trusted.then_some(row.cash_and_equivalents).flatten(),
        net_cash: trusted.then_some(row.net_cash).flatten(),
        free_cash_flow: trusted.then_some(row.free_cash_flow).flatten(),
        net_margin: pct_of(row.net_income, row.revenue),
        gross_margin: pct_of(row.gross_profit, row.revenue),
        operating_margin: pct_of(row.operating_income, row.revenue),
        revenue_growth: pct_change(row.revenue, row.revenue_prior),
        eps: row.eps,
        // 只有明确 FY 才把 EPS 当年化；半年/季度口径禁止 price/eps 反推 PE。
        eps_annualized: Some(row.period_type.as_deref() == Some("FY")),
        period: row.period_label.clone(),
        ..Default::default()
    }
}

fn financials_from_sec(row: &SecFundamentals) -> Financials {
    Financials {
        provider_ok: row.provider_ok(),
        currency: row.currency.clone(),
        revenue: row.revenue,
        gross_profit: row.gross_profit,
        operating_income: row.operating_income,
        net_income: row.net_income,
        operating_cash_flow: row.operating_cash_flow,
        cash_and_equivalents: row.cash_and_equivalents,
        eps: row.eps,
        // Company Facts 补救只会返回严格 TTM 或完整 FY，两者都可用于 PE 口径。
        eps_annualized: row.eps.map(|_| true),
        shares_outstanding: row.shares_outstanding,
        gross_margin: pct_of(row.gross_profit, row.revenue),
        operating_margin: pct_of(row.operating_income, row.revenue),
        net_margin: pct_of(row.net_income, row.revenue),
        period: row.period_end.clone(),
        ..Default::default()
    }
}

fn financials_from_normalized_hk(row: &NormalizedHkFinancials) -> Financials {
    Financials {
        provider_ok: true,
        currency: Some(row.currency.clone()),
        revenue: row.revenue,
        gross_profit: row.gross_profit,
        operating_income: row.operating_income,
        net_income: row.net_income,
        operating_cash_flow: row.operating_cash_flow,
        cash_and_equivalents: row.cash_and_equivalents,
        net_cash: row.net_cash,
        free_cash_flow: row.free_cash_flow,
        eps: row.eps,
        eps_annualized: Some(row.period_type.as_deref() == Some("FY")),
        revenue_growth: pct_change(row.revenue, row.revenue_prior),
        gross_margin: pct_of(row.gross_profit, row.revenue),
        operating_margin: pct_of(row.operating_income, row.revenue),
        net_margin: pct_of(row.net_income, row.revenue),
        period: row.period_end.clone().or_else(|| row.period_label.clone()),
        ..Default::default()
    }
}

impl ResearchPorts for ApiResearchPorts {
    async fn load_company_market(&self, ticker: &str) -> Option<(ResolvedCompany, MarketSnapshot)> {
        let pool = self.state.pool.as_ref()?;
        let company_row = CompanyRepository::new(pool)
            .by_ticker(ticker)
            .await
            .ok()??;
        let market_row = MarketRepository::new(pool)
            .latest_snapshot(ticker)
            .await
            .ok()??;
        let snapshot = market_snapshot_from_rows(&company_row, &market_row);
        let resolved = resolved_company_from_rows(&company_row, Some(&market_row));
        Some((resolved, snapshot))
    }

    async fn refresh_quote(&self, ticker: &str) -> Result<(), String> {
        let quotes = self
            .state
            .quotes
            .as_ref()
            .ok_or_else(|| "quote service unavailable".to_string())?;
        match quotes.refresh(ticker).await {
            Ok(_) => Ok(()),
            Err(error) => {
                warn!(ticker, error = %error, "实时行情未核到");
                Err(error.to_string())
            }
        }
    }

    async fn load_fundamentals(&self, ticker: &str) -> Option<LoadedFundamentals> {
        // 港股走一手财报读模型（`hk_financials`，安全子集：比率 + EPS，绝对值单位不可靠不外传）；
        // FMP 免费档只覆盖美股。缺库/缺行即 `None`——保持"未核到"，绝不占位。
        let normalized = normalize_ticker(ticker);
        if detect_market(&normalized) == Market::Hk {
            let pool = self.state.pool.as_ref()?;
            let row = HkFinancialsRepository::new(pool)
                .latest(&normalized)
                .await
                .ok()??;
            return Some(LoadedFundamentals {
                financials: financials_from_hk_row(&row),
                pe_ttm: None,
                company_name: None,
            });
        }
        let service = self.state.fundamentals.as_ref()?;
        let result = service.fetch(ticker).await;
        if !result.provider_ok {
            return None;
        }
        let row = result.latest()?;
        let company_name = match self.state.fmp_search.as_ref() {
            Some(search) => search
                .exact_us_hit(ticker)
                .await
                .map(|hit| hit.name)
                .filter(|name| !name.is_empty()),
            None => None,
        };
        Some(LoadedFundamentals {
            pe_ttm: row.pe_ttm,
            company_name,
            financials: financials_from_fmp_row(row),
        })
    }

    async fn load_earnings_calendar(&self, ticker: &str) -> Option<EarningsCalendar> {
        let service = self.state.calendar.as_ref()?;
        let row = service.load(ticker).await?;
        row.next_date.is_some().then_some(EarningsCalendar {
            provider_ok: true,
            next_date: row.next_date,
            quarter: row.quarter,
            year: row.year,
            eps_estimate: row.eps_estimate,
            revenue_estimate: row.revenue_estimate,
        })
    }

    async fn load_historical_valuation(&self, ticker: &str) -> Option<HistoricalValuation> {
        let service = self.state.historical_valuation.as_ref()?;
        let summary = service.load(ticker).await?;
        Some(HistoricalValuation {
            percentile: summary.percentile,
            min: summary.min,
            max: summary.max,
            median: summary.median,
            p25: summary.p25,
            p75: summary.p75,
            latest: summary.latest,
        })
    }

    async fn load_peer_anchor(
        &self,
        ticker: &str,
        multiple_type: MultipleType,
    ) -> Option<PeerAnchor> {
        let service = self.state.peers.as_ref()?;
        let summary = service.load(ticker).await?;
        let band = match multiple_type {
            MultipleType::Pe => summary.pe,
            MultipleType::EvSales => summary.ev_sales,
        }?;
        Some(PeerAnchor {
            multiple_type,
            p25: band.p25,
            median: band.median,
            p75: band.p75,
            n: band.n,
            tickers: band.tickers,
        })
    }

    async fn load_recent_filings(&self, ticker: &str) -> Vec<Filing> {
        let Some(service) = self.state.filings.as_ref() else {
            return Vec::new();
        };
        service
            .recent(ticker)
            .await
            .into_iter()
            .map(|filing| Filing {
                form: filing.form,
                filed_date: filing.filed_date,
                source_url: filing.source_url,
            })
            .collect()
    }

    async fn load_web_evidence(
        &self,
        ticker: &str,
        name: Option<&str>,
        question: &str,
    ) -> Vec<Evidence> {
        let Some(service) = self.state.evidence.as_ref() else {
            return Vec::new();
        };
        service
            .search(ticker, name, question)
            .await
            .into_iter()
            .map(|e| Evidence {
                title: e.title,
                url: e.url,
                snippet: e.snippet,
                published_date: e.published_date,
                source_domain: e.source_domain,
            })
            .collect()
    }

    async fn recover_missing_facts(&self, request: FactRecoveryRequest) -> FactRecovery {
        let mut recovery = FactRecovery::default();
        let ticker = normalize_ticker(&request.ticker);
        let market = detect_market(&ticker);
        let needs_financials = request.gaps.iter().any(|gap| {
            matches!(
                gap,
                ResearchGap::FinancialStatements
                    | ResearchGap::TtmEps
                    | ResearchGap::Revenue
                    | ResearchGap::SharesOutstanding
                    | ResearchGap::Valuation
            )
        });

        if request.gaps.contains(&ResearchGap::MarketPrice)
            && self.refresh_quote(&ticker).await.is_ok()
        {
            if let Some((company, snapshot)) = self.load_company_market(&ticker).await {
                recovery.company_name = company.name_zh;
                recovery.market = Some(snapshot);
                recovery.sources.push("quote_fallback".into());
            }
        }

        // Round 1 的财务换源：美股走官方 SEC Company Facts；港股发现并严格解析最近 FY PDF。
        let needs_official_filing = request.gaps.contains(&ResearchGap::RecentFilings);
        if (needs_financials || needs_official_filing) && request.round == 1 {
            match market {
                Market::Us => {
                    if needs_financials {
                        if let Some(service) = self.state.sec_company_facts.as_ref() {
                            match service.fetch(&ticker).await {
                                Ok(row) if row.provider_ok() => {
                                    recovery.company_name = row.company_name.clone();
                                    recovery.fundamentals = Some(LoadedFundamentals {
                                        financials: financials_from_sec(&row),
                                        pe_ttm: None,
                                        company_name: row.company_name,
                                    });
                                    recovery.sources.push("sec_company_facts".into());
                                }
                                Ok(_) => {}
                                Err(error) => {
                                    warn!(ticker, error = %error, "SEC Company Facts 补救未核到");
                                }
                            }
                        }
                    }
                }
                Market::Hk => {
                    if let Some(service) = self.state.hk_annual_reports.as_ref() {
                        match service.recover(&ticker).await {
                            Ok(result) => {
                                recovery.filings.push(Filing {
                                    form: "HKEX FY".into(),
                                    filed_date: result
                                        .announcement
                                        .published_at
                                        .map(|date| date.date_naive().to_string()),
                                    source_url: result.announcement.url,
                                });
                                recovery.sources.push("hkex_annual_report_pdf".into());
                                if let Some(row) = result.financials {
                                    recovery.fundamentals = Some(LoadedFundamentals {
                                        financials: financials_from_normalized_hk(&row),
                                        pe_ttm: None,
                                        company_name: None,
                                    });
                                }
                            }
                            Err(error) => {
                                warn!(ticker, error = %error, "HKEX 年报补救未核到");
                            }
                        }
                    }
                }
                Market::Unsupported => {}
            }
        }

        if market == Market::Us && needs_official_filing {
            recovery.filings = self.load_recent_filings(&ticker).await;
            if !recovery.filings.is_empty() {
                recovery.sources.push("sec_filings_retry".into());
            }
        }

        if request.gaps.contains(&ResearchGap::HistoricalValuation) {
            recovery.historical_valuation = self.load_historical_valuation(&ticker).await;
            if recovery.historical_valuation.is_some() {
                recovery.sources.push("historical_valuation_retry".into());
            }
        }
        if request.gaps.contains(&ResearchGap::PeerComparison) {
            recovery.peer_anchor = self.load_peer_anchor(&ticker, request.multiple_type).await;
            if recovery.peer_anchor.is_some() {
                recovery.sources.push("peer_anchor_retry".into());
            }
        }
        recovery
    }

    async fn load_company_memory(&self, user_id: &str, ticker: &str) -> Option<CompanyMemory> {
        let pool = self.state.pool.as_ref()?;
        let row = CompanyProfileRepository::new(pool)
            .get(user_id, ticker)
            .await
            .ok()??;
        Some(CompanyMemory {
            ticker: row.ticker,
            company_name: row.company_name,
            thesis: row.thesis,
            bull: row.bull.unwrap_or_default(),
            bear: row.bear.unwrap_or_default(),
            monitors: row.monitors.unwrap_or_default(),
            falsifiers: row.falsifiers.unwrap_or_default(),
            profile_md: row.profile_md,
            turn_count: row.turn_count,
        })
    }

    async fn save_company_memory(
        &self,
        user_id: &str,
        ticker: &str,
        update: CompanyMemoryUpdate,
    ) -> Result<(), String> {
        let Some(pool) = self.state.pool.as_ref() else {
            return Ok(());
        };
        CompanyProfileRepository::new(pool)
            .upsert(
                user_id,
                ticker,
                &CompanyProfileUpsert {
                    company_name: update.company_name,
                    valuation_method: update.valuation_method,
                    valuation_bear: update.valuation_bear,
                    valuation_base: update.valuation_base,
                    valuation_bull: update.valuation_bull,
                    valuation_current_price: update.valuation_current_price,
                    profile_md: Some(update.profile_md),
                    ..Default::default()
                },
            )
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn complete_answer(&self, system: &str, user: &str, user_id: &str) -> Option<String> {
        let audit = self
            .state
            .pool
            .as_ref()
            .map(|pool| AuditContext { pool, user_id });
        model_answer(
            system,
            user,
            ModelAnswerOptions::default(),
            self.state.model_provider.as_ref(),
            audit,
        )
        .await
        .map(|generated| generated.content)
    }

    async fn stream_answer(
        &self,
        system: String,
        user: String,
        user_id: String,
    ) -> ModelStreamStart {
        let audit = self.state.pool.as_ref().map(|pool| OwnedAuditContext {
            pool: pool.clone(),
            user_id,
        });
        model_answer_stream(
            system,
            user,
            ModelAnswerOptions::default(),
            self.state.model_provider.clone(),
            audit,
        )
    }

    async fn save_session(
        &self,
        user_id: &str,
        session: PersistResearchSession,
    ) -> Result<String, String> {
        let Some(pool) = &self.state.pool else {
            return Ok(session.id.unwrap_or_else(|| "s_offline".to_string()));
        };
        let save = SaveResearchSession {
            id: session.id,
            ticker: session.ticker,
            company_name: session.company_name,
            question: session.question,
            report_markdown: session.report_markdown,
            decision_panel: session.decision_panel,
            full_research: session.full_research,
            data_sources: session.data_sources,
            turn_count: session.turn_count,
            thread: session.thread,
            ..Default::default()
        };
        ResearchSessionRepository::new(pool)
            .save(user_id, &save)
            .await
            .map_err(|error| error.to_string())
    }

    /// 护栏审计是观测通路：无库或写失败只记日志，绝不让研究本身因此失败或变慢——
    /// 用户问的问题已经答完了，记不上账是我们的问题，不是他的。
    async fn record_guard_audit(&self, audit: GuardAuditRecord) {
        let Some(pool) = &self.state.pool else {
            return;
        };
        let entry = FactGuardAuditEntry {
            ticker: Some(audit.ticker),
            mode: audit.mode.to_string(),
            total: i32::try_from(audit.outcome.view.total).unwrap_or(i32::MAX),
            pass_count: i32::try_from(audit.outcome.view.pass).unwrap_or(i32::MAX),
            soft_count: i32::try_from(audit.outcome.view.soft).unwrap_or(i32::MAX),
            hard_count: i32::try_from(audit.outcome.view.hard).unwrap_or(i32::MAX),
            hard_details: audit
                .outcome
                .hard_details
                .into_iter()
                .map(|(raw, dimension, reason)| FactGuardHardDetail {
                    raw,
                    dimension,
                    reason,
                })
                .collect(),
        };
        if let Err(error) = FactGuardAuditRepository::new(pool).record(&entry).await {
            tracing::warn!(%error, "护栏审计落库失败");
        }
    }

    async fn load_prior_turns(&self, user_id: &str, session_id: &str) -> Vec<PriorTurn> {
        let Some(pool) = &self.state.pool else {
            return Vec::new();
        };
        let Ok(Some(row)) = ResearchSessionRepository::new(pool)
            .get(user_id, session_id)
            .await
        else {
            return Vec::new();
        };
        row.thread_json
            .and_then(|value| serde_json::from_value::<Vec<PriorTurn>>(value).ok())
            .unwrap_or_default()
    }
}

/// 主体识别失败的统一诚实报错——绝不带着空 ticker 往下走（那会产出无事实的臆造面）。
const UNRESOLVED_SUBJECT_MESSAGE: &str =
    "未能从问题中识别出研究对象——请补充公司名称或代码（如 苹果 / AAPL / 0700.HK）。";

/// 对话内自动对比：问题带对比语气、且点名了与主体不同的第二家公司时，返回对比腿候选。
/// 纯规则识别；候选仍要走 `prepare_research_request` 验证/建档后才真正成腿。
fn detect_compare_peer(primary_ticker: &str, question: &str) -> Option<String> {
    if !has_compare_cue(question) {
        return None;
    }
    let primary_key = company_identity_key(primary_ticker);
    match_company_mentions(question)
        .into_iter()
        .map(|mention| mention.ticker)
        .find(|ticker| company_identity_key(ticker) != primary_key)
}

async fn ask(
    State(state): State<AppState>,
    Extension(current_user): Extension<PublicUser>,
    Json(req): Json<AskRequest>,
) -> Result<Json<AskResponse>, ApiError> {
    let req = prepare_research_request(&state, req).await;
    if req.ticker.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            UNRESOLVED_SUBJECT_MESSAGE,
        ));
    }
    let ports = ApiResearchPorts {
        state: state.clone(),
    };
    let outcome = ResearchService::ask(&ports, &current_user.id, req.clone()).await;
    if !outcome.persisted && state.pool.is_some() {
        warn!(ticker = req.ticker, "研究会话落库失败，保留本轮响应");
    }
    Ok(Json(outcome.response))
}

/// 双主体对比研究：`POST /api/compare` —— 两个 ticker 各自走同一条单公司解析/建档管线
/// （`prepare_research_request`），再交给 `ResearchService::compare` 隔离取数、分别护栏。
/// 不落库（对比会话的落库形态待产品判断）。
async fn compare(
    State(state): State<AppState>,
    Extension(current_user): Extension<PublicUser>,
    Json(req): Json<CompareRequest>,
) -> Json<CompareResponse> {
    let primary = prepare_research_request(
        &state,
        AskRequest::minimal(req.question.clone(), req.primary_ticker),
    )
    .await;
    let peer = prepare_research_request(
        &state,
        AskRequest::minimal(req.question.clone(), req.peer_ticker),
    )
    .await;
    let ports = ApiResearchPorts {
        state: state.clone(),
    };
    let outcome = ResearchService::compare(
        &ports,
        &current_user.id,
        req.question,
        primary.ticker,
        peer.ticker,
    )
    .await;
    Json(outcome.response)
}

/// 深度报告：`POST /api/report/generate` —— 与 `/api/ask` 共用同一条取数/建档管线
/// （`prepare_research_request`），交给 `ReportService::generate` 走报告专属提示词/固定结构，
/// 模型不可用或输出过短时退化为本地确定性报告；落库归位同一研究会话（`session_id` 续接）。
async fn report_generate(
    State(state): State<AppState>,
    Extension(current_user): Extension<PublicUser>,
    Json(req): Json<AskRequest>,
) -> Result<Json<ReportGenerateResponse>, ApiError> {
    let req = prepare_research_request(&state, req).await;
    if req.ticker.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            UNRESOLVED_SUBJECT_MESSAGE,
        ));
    }
    let ports = ApiResearchPorts {
        state: state.clone(),
    };
    let outcome = ReportService::generate(&ports, &current_user.id, req.clone()).await;
    if !outcome.persisted && state.pool.is_some() {
        warn!(ticker = req.ticker, "深度报告会话落库失败，保留本轮响应");
    }
    Ok(Json(outcome.response))
}

/// 流式作答：`POST /api/ask/stream` —— 类型化 SSE（meta/stage/delta/guard/final/error）；
/// 仅在干净完成后跑护栏并落库，由 `final.persisted` 报告落库结果。
/// 单事件 SSE 流——主体识别失败等一次性终态用。
fn one_shot_stream(
    event: echo_contracts::ResearchStreamEvent,
) -> tokio::sync::mpsc::Receiver<echo_contracts::ResearchStreamEvent> {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    tokio::spawn(async move {
        let _ = tx.send(event).await;
    });
    rx
}

async fn ask_stream(
    State(state): State<AppState>,
    Extension(current_user): Extension<PublicUser>,
    Json(req): Json<AskRequest>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let req = prepare_research_request(&state, req).await;
    let ports = ApiResearchPorts {
        state: state.clone(),
    };
    let rx = if req.ticker.trim().is_empty() {
        // 主体识别失败：诚实报错，绝不带空 ticker 起研究。
        one_shot_stream(echo_contracts::ResearchStreamEvent::Error(
            echo_contracts::ResearchStreamError {
                message: UNRESOLVED_SUBJECT_MESSAGE.into(),
            },
        ))
    } else if let Some(peer_candidate) = detect_compare_peer(&req.ticker, &req.question) {
        // 对话内自动对比：对比腿走同一条验证/建档管线；验证失败或与主体同司则回落单主体。
        let peer = prepare_research_request(
            &state,
            AskRequest::minimal(req.question.clone(), peer_candidate),
        )
        .await;
        if !peer.ticker.trim().is_empty()
            && company_identity_key(&peer.ticker) != company_identity_key(&req.ticker)
        {
            ResearchService::compare_stream(
                ports,
                current_user.id,
                req.question.clone(),
                req.ticker.clone(),
                peer.ticker,
            )
        } else {
            ResearchService::ask_stream(ports, current_user.id, req)
        }
    } else {
        ResearchService::ask_stream(ports, current_user.id, req)
    };
    let stream = ReceiverStream::new(rx).map(|event: ResearchStreamEvent| {
        let name = event.event_name();
        let data = serde_json::to_string(&event).unwrap_or_else(|_| {
            serde_json::to_string(&ResearchStreamEvent::Error(
                echo_contracts::ResearchStreamError {
                    message: "failed to serialize stream event".into(),
                },
            ))
            .expect("error event serializes")
        });
        Ok(Event::default().event(name).data(data))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub fn router(state: AppState) -> Router {
    let ask_rate_limited = middleware::from_fn_with_state(state.clone(), rate_limit_ask);
    let protected = Router::new()
        .route("/api/account", get(account_get))
        .route("/api/ask", post(ask).route_layer(ask_rate_limited.clone()))
        .route(
            "/api/ask/stream",
            post(ask_stream).route_layer(ask_rate_limited.clone()),
        )
        .route(
            "/api/compare",
            post(compare).route_layer(ask_rate_limited.clone()),
        )
        .route(
            "/api/report/generate",
            post(report_generate).route_layer(ask_rate_limited),
        )
        .route("/api/auth/invite", post(auth_invite))
        .route("/api/companies/search", get(companies_search))
        .route("/api/companies/resolve", get(companies_resolve))
        .route("/api/companies/verify", get(companies_verify))
        .route("/api/watch/list", get(watch_list))
        .route("/api/watch/track", post(watch_track))
        .route("/api/watch/untrack", post(watch_untrack))
        .route(
            "/api/watch/rules",
            get(watch_rules_list)
                .post(watch_rules_create)
                .delete(watch_rules_delete),
        )
        .route("/api/watch/desk", get(watch_desk))
        .route(
            "/api/portfolio",
            get(portfolio_list)
                .post(portfolio_upsert)
                .delete(portfolio_delete),
        )
        .route(
            "/api/preferences",
            get(preferences_get).patch(preferences_update),
        )
        .route("/api/notifications", get(notifications_list))
        .route("/api/notifications/unread", get(notifications_unread))
        .route("/api/notifications/read", post(notifications_read))
        .route(
            "/api/research/sessions",
            get(research_sessions_list).delete(research_sessions_clear),
        )
        .route(
            "/api/research/sessions/:id",
            get(research_session_get).delete(research_session_delete),
        )
        .route("/api/profiles", get(profiles_list))
        .route(
            "/api/profiles/:ticker",
            get(profile_get).put(profile_upsert).delete(profile_delete),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));
    Router::new()
        .route("/health", get(health))
        .route("/healthz", get(health))
        .route("/ready", get(ready))
        .route("/api/auth/login", post(auth_login))
        .route("/api/auth/register", post(auth_register))
        .route("/api/auth/logout", post(auth_logout))
        .route("/api/auth/me", get(auth_me))
        .merge(protected)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            enforce_origin,
        ))
        .layer(DefaultBodyLimit::max(MAX_JSON_BODY_BYTES))
        // 每请求生成一个 tracing span（method+path+status+延迟），是 OTLP 追踪导出的唯一数据
        // 来源——没有这层，echo-observability 配了 OTLP 端点也无 span 可导，是新增导出通路
        // 后必须同 PR 接上的调用方（frozen-table 教训）。显式指定 INFO 级：`DefaultMakeSpan`
        // 缺省是 DEBUG，生产环境默认 `RUST_LOG=info` 会把 span 在到达 OTLP 层之前就过滤掉，
        // 本地曾用真实 OTLP 收集端复现过这个"配了端点但一条 span 都导不出"的坑。
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(tracing::Level::INFO)),
        )
        .with_state(state)
}

pub async fn run() {
    echo_observability::init("echo-api").expect("init tracing");
    let config = ApiConfig::from_env().expect("load echo-api config");
    let listen_addr = config.listen_addr;
    // 配了 DATABASE_URL 就建池（缺行情时兜底 DB 快照）；没配则纯核路径运行——两条路都真跑，
    // 不静默假装接了库。连不上库属硬失败：宁可启动即报，也不带半接的库悄悄降级。
    let pool = match config.database_url.as_deref() {
        Some(url) => {
            let pool = echo_db::connect(url, config.max_connections)
                .await
                .expect("connect DATABASE_URL");
            info!("DATABASE_URL 已连，缺行情将兜底 DB 快照");
            Some(pool)
        }
        None => {
            warn!("未配 DATABASE_URL——纯核路径，只吃请求体数字");
            None
        }
    };
    let quotes = pool.clone().map(|pool| {
        QuoteService::new(pool, config.data_sources.clone()).expect("build quote service")
    });
    let fundamentals = FundamentalsService::new(config.data_sources.clone()).ok();
    let calendar = pool
        .clone()
        .and_then(|pool| CalendarService::new(pool, config.data_sources.clone()).ok());
    let historical_valuation = pool
        .clone()
        .and_then(|pool| HistoricalValuationService::new(pool, config.data_sources.clone()).ok());
    let sec_company_facts = SecCompanyFactsService::new(config.data_sources.clone()).ok();
    let hk_annual_reports = pool
        .clone()
        .and_then(|pool| HkAnnualReportService::new(pool).ok());
    let peers = pool
        .clone()
        .and_then(|pool| PeerService::new(pool, config.data_sources.clone()).ok());
    let filings = pool
        .clone()
        .and_then(|pool| FilingsService::new(pool, config.data_sources.clone()).ok());
    // 有库走 24h `web_evidence` 缓存；无库纯核路径仍可实时检索。缓存故障不会阻断回源。
    let evidence = match pool.clone() {
        Some(pool) => EvidenceService::new_cached(pool, config.data_sources.clone()).ok(),
        None => EvidenceService::new(config.data_sources.clone()).ok(),
    };
    let fmp_search = FmpSearchService::new(config.data_sources.clone()).ok();
    let app = router(AppState {
        pool,
        quotes,
        fundamentals,
        calendar,
        historical_valuation,
        sec_company_facts,
        hk_annual_reports,
        peers,
        filings,
        evidence,
        fmp_search,
        auth_disabled: config.auth_disabled,
        auth_disabled_user_id: config.auth_disabled_user_id,
        secure_cookie: config.secure_cookie,
        model_provider: config.model_provider,
        allowed_origins: config.allowed_origins,
        ask_rate_limit_per_minute: config.ask_rate_limit_per_minute,
    });
    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .expect("bind echo-api");
    info!(address = %listen_addr, "echo-api listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("serve echo-api");
    info!("echo-api 收到停机信号，已排空进行中的请求并退出");
    echo_observability::shutdown();
}

/// SIGTERM（容器编排下发）与 Ctrl+C 均触发优雅停机：`axum::serve` 收到信号后停止接受新连接，
/// 等待存量请求处理完再返回，避免容器滚动更新时中断用户正在进行的研究请求。
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install ctrl+c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use echo_contracts::AnswerSource;
    use tower::ServiceExt;

    #[test]
    fn cookie_parser_is_exact_and_cookie_flags_are_safe() {
        let headers = HeaderMap::from_iter([(
            COOKIE,
            HeaderValue::from_static("other=x; echo_session=abc_123; theme=dark"),
        )]);
        assert_eq!(request_token(&headers), Some("abc_123"));
        let cookie = session_cookie("abc_123", false, true)
            .expect("cookie")
            .to_str()
            .expect("header")
            .to_string();
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        assert!(cookie.contains("Secure"));
    }

    #[test]
    fn hk_financials_withholds_legacy_absolutes_but_keeps_ratios() {
        use rust_decimal::Decimal;
        // 腾讯真实一期：营收 751766000000 / 净利 229801000000（单位可疑但比率抵消单位）。
        let row = HkFinancialsRow {
            currency: Some("CNY".into()),
            period_label: Some("2025 FY".into()),
            period_type: Some("FY".into()),
            unit_label: Some("百萬元".into()),
            source_unit_scale: None,
            amounts_normalized: false,
            revenue: Some(Decimal::from(751_766_000_000_i64)),
            revenue_prior: Some(Decimal::from(660_257_000_000_i64)),
            gross_profit: Some(Decimal::from(400_000_000_000_i64)),
            operating_income: None,
            net_income: Some(Decimal::from(229_801_000_000_i64)),
            eps: Some(Decimal::new(24_749, 3)),
            operating_cash_flow: None,
            cash_and_equivalents: None,
            net_cash: None,
            free_cash_flow: None,
            source_title: Some("年度业绩".into()),
            source_url: Some("https://www1.hkexnews.hk/a.pdf".into()),
            parser_version: None,
        };
        let fin = financials_from_hk_row(&row);
        assert!(fin.provider_ok);
        assert_eq!(fin.currency.as_deref(), Some("CNY"));
        assert_eq!(fin.eps, Some(Decimal::new(24_749, 3)));
        assert_eq!(fin.eps_annualized, Some(true));
        // 净利率 ≈ 30.57%，单位无关、可信。
        let nm = fin.net_margin.expect("net margin");
        assert!(
            nm > Decimal::from(30) && nm < Decimal::from(31),
            "净利率约 30.6%"
        );
        assert!(fin.revenue_growth.is_some(), "增速可算（比率）");
        // 绝对营收/净利/现金流单位不可靠 → 一律不外传，保持未核到。
        assert!(fin.revenue.is_none(), "绝对营收不外传");
        assert!(fin.net_income.is_none(), "绝对净利不外传");
        assert!(fin.free_cash_flow.is_none());
    }

    #[test]
    fn hk_financials_exposes_absolutes_only_with_normalization_proof() {
        use rust_decimal::Decimal;
        let revenue = Decimal::from(10_000_000_000_i64);
        let row = HkFinancialsRow {
            currency: Some("HKD".into()),
            period_label: Some("2025 FY".into()),
            period_type: Some("FY".into()),
            unit_label: Some("百萬元".into()),
            source_unit_scale: Some(Decimal::from(1_000_000)),
            amounts_normalized: true,
            revenue: Some(revenue),
            revenue_prior: Some(Decimal::from(8_000_000_000_i64)),
            gross_profit: Some(Decimal::from(5_000_000_000_i64)),
            operating_income: Some(Decimal::from(1_000_000_000_i64)),
            net_income: Some(Decimal::from(-500_000_000_i64)),
            eps: Some(Decimal::new(-50, 2)),
            operating_cash_flow: Some(Decimal::from(700_000_000_i64)),
            cash_and_equivalents: Some(Decimal::from(2_000_000_000_i64)),
            net_cash: Some(Decimal::from(1_500_000_000_i64)),
            free_cash_flow: Some(Decimal::from(600_000_000_i64)),
            source_title: Some("年度业绩".into()),
            source_url: Some("https://www1.hkexnews.hk/a.pdf".into()),
            parser_version: Some("hkex-structured-v1".into()),
        };
        let fin = financials_from_hk_row(&row);
        assert_eq!(fin.revenue, Some(revenue));
        assert_eq!(fin.net_income, Some(Decimal::from(-500_000_000_i64)));
        assert_eq!(fin.net_cash, Some(Decimal::from(1_500_000_000_i64)));
        assert_eq!(fin.eps_annualized, Some(true));
    }

    #[test]
    fn quiet_hours_parser_is_ascii_exact_and_panic_free() {
        assert!(valid_hhmm("00:00"));
        assert!(valid_hhmm("23:59"));
        assert!(!valid_hhmm("24:00"));
        assert!(!valid_hhmm("12:60"));
        assert!(!valid_hhmm("九:00"));
        assert!(!valid_hhmm("1:00"));
    }

    #[tokio::test]
    async fn health_and_local_auth_are_available_without_database() {
        let app = router(AppState::without_database());
        let health = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("health");
        assert_eq!(health.status(), StatusCode::OK);

        let healthz = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("healthz");
        assert_eq!(healthz.status(), StatusCode::OK);

        let me = app
            .oneshot(
                Request::builder()
                    .uri("/api/auth/me")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("me");
        assert_eq!(me.status(), StatusCode::OK);
        let body = to_bytes(me.into_body(), 64 * 1024).await.expect("body");
        let response: AuthMeResponse = serde_json::from_slice(&body).expect("contract");
        assert_eq!(response.user.expect("local user").id, "local");
        assert!(!response.multi_user);
    }

    #[tokio::test]
    async fn protected_ask_receives_local_tenant_in_dbless_mode() {
        let request = AskRequest::minimal("苹果估值？", "AAPL");
        let response = router(AppState::without_database())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/ask")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&request).expect("json")))
                    .expect("request"),
            )
            .await
            .expect("ask");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 256 * 1024)
            .await
            .expect("body");
        let answer: AskResponse = serde_json::from_slice(&body).expect("shared contract");
        assert_eq!(answer.ticker, "AAPL");
        assert_eq!(answer.answer_source, AnswerSource::Unavailable);
    }

    #[tokio::test]
    async fn readiness_is_ok_without_database() {
        let response = router(AppState::without_database())
            .oneshot(
                Request::builder()
                    .uri("/ready")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("ready");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn mismatched_origin_is_rejected_on_mutating_requests() {
        let request = AskRequest::minimal("苹果估值？", "AAPL");
        let response = router(AppState::without_database())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/ask")
                    .header("content-type", "application/json")
                    .header("origin", "https://evil.example")
                    .body(Body::from(serde_json::to_vec(&request).expect("json")))
                    .expect("request"),
            )
            .await
            .expect("ask");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn matching_origin_is_allowed_through() {
        let request = AskRequest::minimal("苹果估值？", "AAPL");
        let response = router(AppState::without_database())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/ask")
                    .header("content-type", "application/json")
                    .header("origin", "http://localhost:5191")
                    .body(Body::from(serde_json::to_vec(&request).expect("json")))
                    .expect("request"),
            )
            .await
            .expect("ask");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    #[ignore = "需要隔离 DATABASE_URL；验证 Rust 认证、RLS 会话和保护路由"]
    async fn live_register_session_logout_round_trip() {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let pool = echo_db::connect(&database_url, 3).await.expect("connect");
        if std::env::var("ECHO_SKIP_TEST_MIGRATE").ok().as_deref() != Some("1") {
            echo_db::migrate(&pool).await.expect("migrate");
        }
        let auth = AuthService::new(&pool);
        let owner = auth
            .create_owner("owner@example.com", "owner-password", Some("Owner".into()))
            .await
            .expect("owner");
        let invite = auth
            .create_invite(&owner, Some("integration"))
            .await
            .expect("invite");
        let app = router(AppState {
            pool: Some(pool),
            quotes: None,
            fundamentals: None,
            calendar: None,
            historical_valuation: None,
            sec_company_facts: None,
            hk_annual_reports: None,
            peers: None,
            filings: None,
            evidence: None,
            fmp_search: None,
            auth_disabled: false,
            auth_disabled_user_id: "local".into(),
            secure_cookie: false,
            model_provider: None,
            allowed_origins: vec!["http://localhost:5191".into()],
            ask_rate_limit_per_minute: 20,
        });

        let register = AuthRegisterRequest {
            invite,
            username: "member@example.com".into(),
            password: "member-password".into(),
            display_name: Some("Member".into()),
        };
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/register")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&register).expect("json")))
                    .expect("request"),
            )
            .await
            .expect("register");
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response
            .headers()
            .get(SET_COOKIE)
            .expect("session cookie")
            .to_str()
            .expect("cookie")
            .split(';')
            .next()
            .expect("cookie pair")
            .to_string();

        let ask = AskRequest::minimal("腾讯估值？", "0700.HK");
        let protected = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/ask")
                    .header("content-type", "application/json")
                    .header(COOKIE, &cookie)
                    .body(Body::from(serde_json::to_vec(&ask).expect("json")))
                    .expect("request"),
            )
            .await
            .expect("protected ask");
        assert_eq!(protected.status(), StatusCode::OK);

        let logout = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/logout")
                    .header(COOKIE, &cookie)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("logout");
        assert_eq!(logout.status(), StatusCode::OK);
        assert!(
            logout.headers()[SET_COOKIE]
                .to_str()
                .expect("clear cookie")
                .contains("Max-Age=0")
        );

        let rejected = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/ask")
                    .header("content-type", "application/json")
                    .header(COOKIE, cookie)
                    .body(Body::from(serde_json::to_vec(&ask).expect("json")))
                    .expect("request"),
            )
            .await
            .expect("rejected ask");
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
    }
}
