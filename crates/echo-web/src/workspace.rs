use crate::company_search::TickerSearchInput;
use crate::dialog::confirm_destructive;
use crate::format;
use crate::icons::{EchoArt, EchoMark, Icon};
use crate::{api, profiles::ProfilesSection, research::ResearchPage};
use echo_contracts::{
    AccountResponse, AuthInviteRequest, AuthInviteResponse, AuthLoginRequest, AuthLogoutResponse,
    AuthRegisterRequest, AuthUserResponse, ChangedCountResponse, CompanySearchItem, Decimal,
    DeskResponse, MutationResponse, NotificationReadRequest, NotificationsListResponse,
    PortfolioListResponse, PortfolioUpsertRequest, PreferencesResponse, PreferencesUpdateRequest,
    PublicUser, UnreadResponse, UserRole, WatchListResponse, WatchMutationRequest,
    WatchRuleCreateRequest,
};
use leptos::*;

/// 资料库内的三个切面——同一批公司的自选监控、持仓与研究档案。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LibraryTab {
    Watch,
    Portfolio,
    Profiles,
}

impl LibraryTab {
    const fn label(self) -> &'static str {
        match self {
            Self::Watch => "自选与监控",
            Self::Portfolio => "持仓",
            Self::Profiles => "研究档案",
        }
    }

    /// 分段滑块的位置类名——由 tab 自己决定，不在视图里手写九种组合。
    const fn indicator_class(self) -> &'static str {
        match self {
            Self::Watch => "segmented-indicator is-watch",
            Self::Portfolio => "segmented-indicator is-portfolio",
            Self::Profiles => "segmented-indicator is-profiles",
        }
    }
}

/// 进入研究页的方式：续接某个历史会话，或带着一个预置研究对象开新会话。
/// 两者都可为空——那就是一个干净的新研究。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ResearchEntry {
    session: Option<String>,
    ticker: Option<String>,
}

impl ResearchEntry {
    fn session(id: Option<String>) -> Self {
        Self {
            session: id,
            ticker: None,
        }
    }

    fn with_ticker(ticker: String) -> Self {
        Self {
            session: None,
            ticker: Some(ticker),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Page {
    /// 研究页——可选携带会话 id，对应深链 `/research/:session_id`。
    Research(ResearchEntry),
    Library(LibraryTab),
    Settings,
}

impl Page {
    #[cfg(target_arch = "wasm32")]
    fn path(&self) -> String {
        match self {
            Self::Research(entry) => match &entry.session {
                Some(id) => format!("/research/{id}"),
                None => "/research".to_string(),
            },
            Self::Library(LibraryTab::Watch) => "/library".to_string(),
            Self::Library(LibraryTab::Portfolio) => "/library/portfolio".to_string(),
            Self::Library(LibraryTab::Profiles) => "/library/profiles".to_string(),
            Self::Settings => "/settings".to_string(),
        }
    }

    const fn label(&self) -> &'static str {
        match self {
            Self::Research(_) => "研究",
            Self::Library(_) => "资料库",
            Self::Settings => "设置",
        }
    }

    /// 导航栏高亮只看落在哪个 tab，不看研究页携带的具体会话 id / 资料库切面。
    fn same_tab(&self, other: &Page) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

#[cfg(target_arch = "wasm32")]
fn page_from_path(path: &str) -> Page {
    if let Some(rest) = path.strip_prefix("/research/") {
        let id = rest.trim_matches('/');
        return Page::Research(ResearchEntry::session(
            (!id.is_empty()).then(|| id.to_string()),
        ));
    }
    match path {
        // 旧的独立页面路径一并归位到资料库对应切面，深链/书签不失效。
        "/library" | "/watch" => Page::Library(LibraryTab::Watch),
        "/library/portfolio" | "/portfolio" => Page::Library(LibraryTab::Portfolio),
        "/library/profiles" | "/profiles" => Page::Library(LibraryTab::Profiles),
        "/settings" => Page::Settings,
        _ => Page::Research(ResearchEntry::default()),
    }
}

#[cfg(target_arch = "wasm32")]
fn initial_page() -> Page {
    page_from_path(&leptos::window().location().pathname().unwrap_or_default())
}

#[cfg(not(target_arch = "wasm32"))]
fn initial_page() -> Page {
    Page::Research(ResearchEntry::default())
}

fn navigate(set_page: WriteSignal<Page>, page: Page) {
    #[cfg(target_arch = "wasm32")]
    let path = page.path();
    set_page.set(page);
    #[cfg(target_arch = "wasm32")]
    if let Ok(history) = leptos::window().history() {
        let _ = history.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&path));
    }
}

/// 资料库内部切面共用同一个页面实例：切换时只同步 URL，不再次挂载整页。
/// 浏览器前进/后退仍由 popstate 负责恢复对应切面。
fn push_library_path(tab: LibraryTab) {
    #[cfg(target_arch = "wasm32")]
    {
        let path = Page::Library(tab).path();
        if let Ok(history) = leptos::window().history() {
            let _ = history.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&path));
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = tab;
    }
}

/// 监听浏览器前进/后退——`navigate` 只 `push_state` 不够，URL 变了页面得跟着变，
/// 否则后退键在地址栏里动了但视图纹丝不动。
#[cfg(target_arch = "wasm32")]
fn install_popstate_listener(set_page: WriteSignal<Page>) {
    let handle = leptos::window_event_listener(leptos::ev::popstate, move |_| {
        let path = leptos::window().location().pathname().unwrap_or_default();
        set_page.set(page_from_path(&path));
    });
    on_cleanup(move || handle.remove());
}

#[cfg(not(target_arch = "wasm32"))]
fn install_popstate_listener(_set_page: WriteSignal<Page>) {}

/// Escape 关闭顶栏浮层——通知面板与账户菜单共用。
#[cfg(target_arch = "wasm32")]
fn install_escape_close(open: ReadSignal<bool>, set_open: WriteSignal<bool>) {
    let handle = leptos::window_event_listener(leptos::ev::keydown, move |event| {
        if event.key() == "Escape" && open.get_untracked() {
            set_open.set(false);
        }
    });
    on_cleanup(move || handle.remove());
}

#[cfg(not(target_arch = "wasm32"))]
fn install_escape_close(_open: ReadSignal<bool>, _set_open: WriteSignal<bool>) {}

fn user_initials(name: &str) -> String {
    let words: Vec<_> = name.split_whitespace().collect();
    if words.len() > 1 {
        return words
            .iter()
            .take(2)
            .filter_map(|word| word.chars().next())
            .flat_map(char::to_uppercase)
            .collect();
    }
    name.chars().take(2).flat_map(char::to_uppercase).collect()
}

fn subscription_status_label(status: &str) -> &'static str {
    match status {
        "active" => "已订阅",
        "trialing" => "试用中",
        "canceled" => "已取消",
        _ => "订阅信息",
    }
}

#[component]
pub fn Workspace(user: PublicUser, on_auth_changed: Callback<()>) -> impl IntoView {
    let (page, set_page) = create_signal(initial_page());
    let (account_open, set_account_open) = create_signal(false);
    let stage_ref = create_node_ref::<html::Section>();
    install_popstate_listener(set_page);
    install_escape_close(account_open, set_account_open);
    #[cfg(target_arch = "wasm32")]
    create_effect(move |_| {
        let _ = page.get();
        request_animation_frame(move || {
            if let Some(stage) = stage_ref.get_untracked() {
                stage.set_scroll_top(0);
            }
        });
    });
    let logout = create_action(|_: &()| async {
        api::post::<_, AuthLogoutResponse>("/api/auth/logout", &serde_json::json!({})).await
    });
    let account = create_resource(
        || (),
        |_| async { api::get::<AccountResponse>("/api/account").await },
    );
    create_effect(move |_| {
        if matches!(logout.value().get(), Some(Ok(_))) {
            on_auth_changed.call(());
        }
    });
    let display_name = user
        .display_name
        .clone()
        .unwrap_or_else(|| user.username.clone());
    let avatar_letters = user_initials(&display_name);
    let account_name = display_name.clone();
    let account_username = user.username.clone();
    let on_research_navigate = Callback::new(move |session_id: Option<String>| {
        navigate(set_page, Page::Research(ResearchEntry::session(session_id)))
    });
    let on_research_ticker = Callback::new(move |ticker: String| {
        navigate(set_page, Page::Research(ResearchEntry::with_ticker(ticker)))
    });

    view! {
        <div class="app-shell">
            <header class="echo-topbar">
                <button
                    class="topbar-brand"
                    aria-label="回到研究台"
                    on:click=move |_| navigate(set_page, Page::Research(ResearchEntry::default()))
                >
                    <span class="echo-brand-mark"><EchoMark /></span>
                    <span class="echo-brand-copy">
                        <span class="echo-brand-name">"ECHO"</span>
                        <span class="echo-brand-sub">"RESEARCH"</span>
                    </span>
                </button>
                <nav class="workspace-nav" aria-label="主导航">
                    {[
                        Page::Research(ResearchEntry::default()),
                        Page::Library(LibraryTab::Watch),
                        Page::Settings,
                    ]
                        .into_iter()
                        .map(|item| {
                            let item_for_class = item.clone();
                            let item_for_current = item.clone();
                            let item_for_click = item.clone();
                            view! {
                                <button
                                    class=move || if page.get().same_tab(&item_for_class) { "nav-item is-active" } else { "nav-item" }
                                    aria-current=move || if page.get().same_tab(&item_for_current) { "page" } else { "false" }
                                    on:click=move |_| navigate(set_page, item_for_click.clone())
                                >{item.label()}</button>
                            }
                        })
                        .collect_view()}
                </nav>
                <div class="topbar-actions">
                    <NotificationsPanel />
                    <div class="rail-account">
                        <button
                            class=move || if account_open.get() { "account-card is-open" } else { "account-card" }
                            aria-label="账户菜单"
                            aria-expanded=move || account_open.get()
                            on:click=move |_| set_account_open.update(|value| *value = !*value)
                        >
                            <span class="user-avatar" aria-hidden="true">{avatar_letters.clone()}</span>
                            // 只显示名字。"FOUNDER ACCESS / RESEARCH ACCESS" 这类头衔
                            // 不改变用户能做什么，是纯装饰。
                            <span class="account-copy">
                                <strong>{account_name.clone()}</strong>
                                <small>{move || {
                                    account
                                        .get()
                                        .and_then(Result::ok)
                                        .and_then(|response| response.subscription)
                                        .map(|subscription| format!(
                                            "{} · {}",
                                            subscription.plan_name,
                                            subscription_status_label(&subscription.status)
                                        ))
                                        .unwrap_or_else(|| "ECHO 会员".to_string())
                                }}</small>
                            </span>
                            <span class="account-chevron" aria-hidden="true"><Icon name="chevron-down" /></span>
                        </button>
                        {move || account_open.get().then(|| view! {
                            <div class="dismiss-layer" on:click=move |_| set_account_open.set(false)></div>
                            <div class="account-menu" role="menu">
                                <div class="account-menu-profile">
                                    <span class="user-avatar" aria-hidden="true">{avatar_letters.clone()}</span>
                                    <span>
                                        <strong>{account_name.clone()}</strong>
                                        <small>{account_username.clone()}</small>
                                    </span>
                                </div>
                                {move || {
                                    account
                                        .get()
                                        .and_then(Result::ok)
                                        .and_then(|response| response.subscription)
                                        .map(|subscription| {
                                            let status_class =
                                                format!("subscription-status is-{}", subscription.status);
                                            let status =
                                                subscription_status_label(&subscription.status);
                                            let period_end =
                                                format::timestamp(&subscription.current_period_end);
                                            view! {
                                                <div class="account-subscription">
                                                    <div class="subscription-heading">
                                                        <span>"当前订阅"</span>
                                                        <b class=status_class>{status}</b>
                                                    </div>
                                                    <strong>{subscription.plan_name}</strong>
                                                    <p>{format!(
                                                        "有效期至 {period_end} · 每日 {} 次研究调用",
                                                        subscription.max_daily_calls
                                                    )}</p>
                                                    <div class="subscription-features">
                                                        {subscription
                                                            .features
                                                            .into_iter()
                                                            .take(4)
                                                            .map(|feature| view! { <span>{feature}</span> })
                                                            .collect_view()}
                                                    </div>
                                                </div>
                                            }
                                        })
                                }}
                                <div class="account-menu-actions">
                                    <button role="menuitem" on:click=move |_| {
                                        set_account_open.set(false);
                                        navigate(set_page, Page::Settings);
                                    }>
                                        <span>"通知与偏好"</span><b aria-hidden="true">"→"</b>
                                    </button>
                                    <button
                                        class="is-danger account-logout"
                                        role="menuitem"
                                        disabled=move || logout.pending().get()
                                        on:click=move |_| logout.dispatch(())
                                    >
                                        <span>{move || if logout.pending().get() { "正在安全退出…" } else { "退出当前账号" }}</span>
                                        <b aria-hidden="true">"↗"</b>
                                    </button>
                                </div>
                                {move || logout.value().get().and_then(Result::err).map(|message| view! {
                                    <p class="account-menu-error" role="alert">{message}</p>
                                })}
                            </div>
                        })}
                    </div>
                </div>
            </header>
            <section node_ref=stage_ref class="workspace-stage">
                {move || match page.get() {
                    Page::Research(entry) => view! {
                        <ResearchPage
                            initial_session=entry.session
                            initial_ticker=entry.ticker
                            on_navigate=on_research_navigate
                        />
                    }.into_view(),
                    Page::Library(tab) => view! {
                        <LibraryPage
                            tab=tab
                            on_tab=Callback::new(push_library_path)
                            on_research=on_research_ticker
                        />
                    }.into_view(),
                    Page::Settings => view! { <SettingsPage role=user.role /> }.into_view(),
                }}
            </section>
        </div>
    }
}

#[derive(Clone)]
struct AuthSubmission {
    register: bool,
    username: String,
    password: String,
    invite: String,
    display_name: String,
}

#[cfg(target_arch = "wasm32")]
fn finish_auth_transition(callback: Callback<()>) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;

    let closure = Closure::once(move || callback.call(()));
    let _ = leptos::window().set_timeout_with_callback_and_timeout_and_arguments_0(
        closure.as_ref().unchecked_ref(),
        560,
    );
    closure.forget();
}

#[cfg(not(target_arch = "wasm32"))]
fn finish_auth_transition(callback: Callback<()>) {
    callback.call(());
}

#[component]
pub fn LoginPage(on_authenticated: Callback<()>) -> impl IntoView {
    let (register, set_register) = create_signal(false);
    let (username, set_username) = create_signal(String::new());
    let (password, set_password) = create_signal(String::new());
    let (password_visible, set_password_visible) = create_signal(false);
    let (invite, set_invite) = create_signal(String::new());
    let (display_name, set_display_name) = create_signal(String::new());
    let (transitioning, set_transitioning) = create_signal(false);
    let submit = create_action(|input: &AuthSubmission| {
        let input = input.clone();
        async move {
            if input.register {
                api::post::<_, AuthUserResponse>(
                    "/api/auth/register",
                    &AuthRegisterRequest {
                        invite: input.invite,
                        username: input.username,
                        password: input.password,
                        display_name: (!input.display_name.trim().is_empty())
                            .then_some(input.display_name),
                    },
                )
                .await
            } else {
                api::post::<_, AuthUserResponse>(
                    "/api/auth/login",
                    &AuthLoginRequest {
                        username: input.username,
                        password: input.password,
                    },
                )
                .await
            }
        }
    });
    create_effect(move |_| {
        if matches!(submit.value().get(), Some(Ok(_))) && !transitioning.get_untracked() {
            set_transitioning.set(true);
            finish_auth_transition(on_authenticated);
        }
    });
    let can_submit = create_memo(move |_| {
        let username = username.get();
        let password = password.get();
        !username.trim().is_empty()
            && !password.is_empty()
            && (!register.get()
                || (password.chars().count() >= 8 && !invite.get().trim().is_empty()))
    });
    let dispatch = Callback::new(move |_| {
        if submit.pending().get_untracked() || !can_submit.get_untracked() {
            return;
        }
        submit.dispatch(AuthSubmission {
            register: register.get(),
            username: username.get().trim().to_string(),
            password: password.get(),
            invite: invite.get().trim().to_string(),
            display_name: display_name.get().trim().to_string(),
        });
    });
    // 切换登录/注册时清掉上一模式的过期错误——不让"密码至少 8 位"留在登录态里。
    let switch_mode = move |to_register: bool| {
        set_register.set(to_register);
        set_password_visible.set(false);
        submit.value().set(None);
    };

    view! {
        <main class=move || if transitioning.get() { "auth-page is-leaving" } else { "auth-page" }>
            <div class="auth-brand-lockup">
                <span class="echo-brand-mark auth-mark"><EchoMark /></span>
                <span><strong>"ECHO"</strong><small>"RESEARCH INTELLIGENCE"</small></span>
            </div>
            <section class="auth-story">
                <EchoArt class="auth-echo-art" />
                <div class="auth-story-inner">
                    <h1>"听见数据背后，"<br/><em>"真正重要的答案。"</em></h1>
                    <p class="auth-story-copy">"把实时行情、原始披露、财务数据与研究上下文连成一条可验证的证据链，让每一次判断更清晰、更有依据。"</p>
                </div>
            </section>
            <form
                class=move || if transitioning.get() { "auth-card is-success" } else { "auth-card" }
                aria-label=move || if register.get() { "邀请码注册" } else { "账户登录" }
                on:submit=move |event| {
                    event.prevent_default();
                    dispatch.call(());
                }
            >
                <div class="auth-card-head">
                    <p class="auth-card-kicker">{move || if register.get() { "JOIN ECHO" } else { "WELCOME BACK" }}</p>
                    <h2>{move || if register.get() { "创建研究空间" } else { "登录 ECHO 工作区" }}</h2>
                </div>
                <div class="auth-tabs" role="tablist" aria-label="账户入口">
                    <button
                        type="button"
                        role="tab"
                        aria-selected=move || !register.get()
                        class=move || if !register.get() { "is-active" } else { "" }
                        on:click=move |_| switch_mode(false)
                    >"登录"</button>
                    <button
                        type="button"
                        role="tab"
                        aria-selected=move || register.get()
                        class=move || if register.get() { "is-active" } else { "" }
                        on:click=move |_| switch_mode(true)
                    >"邀请码注册"</button>
                </div>
                <div class="auth-field">
                    <label for="auth-identity">"邮箱 / 用户名"</label>
                    <div class="auth-input-shell">
                        <input
                            id="auth-identity"
                            name="identity"
                            autocomplete="username"
                            autocapitalize="none"
                            spellcheck="false"
                            placeholder="name@example.com"
                            prop:value=username
                            on:input=move |event| {
                                submit.value().set(None);
                                set_username.set(event_target_value(&event));
                            }
                        />
                    </div>
                    {move || register.get().then(|| view! {
                        <span class="auth-field-hint">"可使用邮箱，或 3–24 位字母数字用户名"</span>
                    })}
                </div>
                <div class="auth-field">
                    <label for="auth-password">"密码"</label>
                    <div class="auth-input-shell">
                        <input
                            id="auth-password"
                            name="password"
                            type=move || if password_visible.get() { "text" } else { "password" }
                            autocomplete=move || if register.get() { "new-password" } else { "current-password" }
                            placeholder="输入密码"
                            prop:value=password
                            on:input=move |event| {
                                submit.value().set(None);
                                set_password.set(event_target_value(&event));
                            }
                        />
                        <button
                            class="auth-password-toggle"
                            type="button"
                            aria-label=move || if password_visible.get() { "隐藏密码" } else { "显示密码" }
                            aria-pressed=move || password_visible.get()
                            on:click=move |_| set_password_visible.update(|visible| *visible = !*visible)
                        >
                            {move || if password_visible.get() {
                                view! { <Icon name="eye-off" /> }
                            } else {
                                view! { <Icon name="eye" /> }
                            }}
                        </button>
                    </div>
                    {move || register.get().then(|| view! {
                        <span class=move || {
                            if !password.get().is_empty() && password.get().chars().count() < 8 {
                                "auth-field-hint is-warning"
                            } else {
                                "auth-field-hint"
                            }
                        }>"注册密码至少 8 位"</span>
                    })}
                </div>
                {move || register.get().then(|| view! {
                    <div class="register-extra">
                        <div class="auth-field">
                            <label for="auth-display-name">"显示名称（选填）"</label>
                            <div class="auth-input-shell">
                                <input
                                    id="auth-display-name"
                                    name="display-name"
                                    autocomplete="name"
                                    placeholder="你希望显示的名字"
                                    prop:value=display_name
                                    on:input=move |event| {
                                        submit.value().set(None);
                                        set_display_name.set(event_target_value(&event));
                                    }
                                />
                            </div>
                        </div>
                        <div class="auth-field">
                            <label for="auth-invite">"邀请码"</label>
                            <div class="auth-input-shell">
                                <input
                                    id="auth-invite"
                                    name="invite"
                                    autocomplete="one-time-code"
                                    autocapitalize="none"
                                    spellcheck="false"
                                    placeholder="输入邀请码"
                                    prop:value=invite
                                    on:input=move |event| {
                                        submit.value().set(None);
                                        set_invite.set(event_target_value(&event));
                                    }
                                />
                            </div>
                        </div>
                    </div>
                })}
                <div class="auth-feedback" aria-live="polite">
                    {move || submit.value().get().and_then(Result::err).map(|message| view! {
                        <p class="form-error" role="alert"><span aria-hidden="true">"!"</span>{message}</p>
                    })}
                </div>
                <button
                    type="submit"
                    class="primary-button auth-submit"
                    aria-busy=move || submit.pending().get()
                    disabled=move || submit.pending().get() || !can_submit.get()
                >
                    <span class="auth-submit-label">
                        {move || if submit.pending().get() { "正在验证" } else if register.get() { "创建账号" } else { "进入研究台" }}
                    </span>
                    {move || if submit.pending().get() {
                        view! {
                            <span class="auth-submit-arrow is-loading" aria-hidden="true">
                                <i></i><i></i><i></i>
                            </span>
                        }
                    } else {
                        view! {
                            <span class="auth-submit-arrow" aria-hidden="true">"→"</span>
                        }
                    }}
                </button>
                {move || transitioning.get().then(|| view! {
                    <span class="auth-success-bloom" aria-hidden="true">
                        <Icon name="check" />
                    </span>
                })}
            </form>
        </main>
    }
}

#[component]
fn NotificationsPanel() -> impl IntoView {
    let (open, set_open) = create_signal(false);
    install_escape_close(open, set_open);
    let unread = create_resource(
        || (),
        |_| api::get::<UnreadResponse>("/api/notifications/unread"),
    );
    let notifications = create_resource(
        || (),
        |_| api::get::<NotificationsListResponse>("/api/notifications?limit=20"),
    );
    let mark_all = create_action(|_: &()| async {
        api::post::<_, ChangedCountResponse>(
            "/api/notifications/read",
            &NotificationReadRequest { id: None },
        )
        .await
    });
    create_effect(move |_| {
        if matches!(mark_all.value().get(), Some(Ok(_))) {
            unread.refetch();
            notifications.refetch();
        }
    });
    view! {
        <div class="notification-center">
            <button
                class="notification-button"
                aria-label=move || match unread.get().and_then(Result::ok) {
                    Some(value) if value.unread > 0 => format!("通知，{} 条未读", value.unread),
                    _ => "通知".to_string(),
                }
                aria-expanded=move || open.get()
                on:click=move |_| set_open.update(|value| *value = !*value)
            >
                <Icon name="bell" />
                {move || unread.get().and_then(Result::ok).filter(|value| value.unread > 0).map(|value| view! {
                    <span aria-hidden="true">{value.unread}</span>
                })}
            </button>
            {move || open.get().then(|| view! {
                <div class="dismiss-layer" on:click=move |_| set_open.set(false)></div>
                <div class="notification-popover">
                    <div class="popover-head">
                        <strong>"通知"</strong>
                        <button on:click=move |_| mark_all.dispatch(())>"全部标为已读"</button>
                    </div>
                    {move || match notifications.get() {
                        None => loading_view(),
                        Some(Err(error)) => error_view(error),
                        Some(Ok(data)) if data.notifications.is_empty() => empty_view(
                            "暂无通知",
                            "监控规则触发或研究有新结论时，通知会自动送到这里。",
                        ),
                        Some(Ok(data)) => data.notifications.into_iter().map(|item| view! {
                            <article class=if item.read_at.is_some() { "notice is-read" } else { "notice" }>
                                <i aria-hidden="true"></i>
                                <div>
                                    <strong>{item.title}</strong>
                                    <p>{item.body}</p>
                                    <time>{format::timestamp(&item.created_at)}</time>
                                </div>
                            </article>
                        }).collect_view().into_view(),
                    }}
                </div>
            })}
        </div>
    }
}

/// 资料库页——自选/持仓/档案三个切面的统一容器，滑块跟随选中项移动。
#[component]
fn LibraryPage(
    tab: LibraryTab,
    on_tab: Callback<LibraryTab>,
    on_research: Callback<String>,
) -> impl IntoView {
    let (active_tab, set_active_tab) = create_signal(tab);
    view! {
        <PageHeader title="资料库" detail="自选、持仓与研究档案，一处掌控投资研究的关键脉络。" />
        <main class="page-content library-page-content">
            <div class="segmented" role="tablist" aria-label="资料库切面">
                <span class=move || active_tab.get().indicator_class() aria-hidden="true"></span>
                {[LibraryTab::Watch, LibraryTab::Portfolio, LibraryTab::Profiles]
                    .into_iter()
                    .map(|item| view! {
                        <button
                            class=move || if item == active_tab.get() { "segmented-item is-active" } else { "segmented-item" }
                            role="tab"
                            aria-selected=move || if item == active_tab.get() { "true" } else { "false" }
                            on:click=move |_| {
                                if item == active_tab.get_untracked() {
                                    return;
                                }
                                set_active_tab.set(item);
                                on_tab.call(item);
                            }
                        >{item.label()}</button>
                    })
                    .collect_view()}
            </div>
            {move || match active_tab.get() {
                LibraryTab::Watch => view! { <WatchSection on_research=on_research /> }.into_view(),
                LibraryTab::Portfolio => view! { <PortfolioSection /> }.into_view(),
                LibraryTab::Profiles => view! { <ProfilesSection /> }.into_view(),
            }}
        </main>
    }
}

#[derive(Clone)]
struct WatchAction {
    track: bool,
    ticker: String,
    company_name: Option<String>,
}

fn market_label(ticker: &str) -> &'static str {
    if ticker.ends_with(".HK") {
        "港股"
    } else if ticker.ends_with(".SH") || ticker.ends_with(".SZ") {
        "A 股"
    } else {
        "美股"
    }
}

fn ticker_initial(ticker: &str) -> String {
    ticker
        .chars()
        .find(char::is_ascii_alphabetic)
        .unwrap_or('E')
        .to_uppercase()
        .to_string()
}

#[component]
fn WatchSection(on_research: Callback<String>) -> impl IntoView {
    let (ticker, set_ticker) = create_signal(String::new());
    let (company_name, set_company_name) = create_signal(String::new());
    let entries = create_resource(|| (), |_| api::get::<WatchListResponse>("/api/watch/list"));
    let mutate = create_action(|input: &WatchAction| {
        let input = input.clone();
        async move {
            api::post::<_, MutationResponse>(
                if input.track {
                    "/api/watch/track"
                } else {
                    "/api/watch/untrack"
                },
                &WatchMutationRequest {
                    ticker: input.ticker,
                    company_name: input.company_name,
                },
            )
            .await
        }
    });
    create_effect(move |_| {
        if matches!(mutate.value().get(), Some(Ok(_))) {
            entries.refetch();
            set_ticker.set(String::new());
            set_company_name.set(String::new());
        }
    });
    let submit = move || {
        let value = ticker.get().trim().to_string();
        if value.is_empty() {
            return;
        }
        mutate.dispatch(WatchAction {
            track: true,
            ticker: value,
            company_name: (!company_name.get().trim().is_empty()).then(|| company_name.get()),
        });
    };
    view! {
        <section class="library-section watch-section">
            <section class="data-panel watchlist-panel">
                <div class="panel-titlebar">
                    <h2>"关注公司"</h2>
                </div>
                <div class="watchlist-form">
                    <div class="form-grid watch-form-grid">
                        <label class="form-field">
                            <span>"股票代码"</span>
                            <TickerSearchInput
                                value=ticker
                                set_value=set_ticker
                                placeholder="输入代码或公司名，如 0700.HK / 腾讯"
                                on_pick=Callback::new(move |item: CompanySearchItem| {
                                    // 选中建议时顺手补上公司名，省得用户再打一遍已知的东西。
                                    if company_name.get_untracked().trim().is_empty() {
                                        set_company_name.set(item.name_zh);
                                    }
                                })
                                on_submit=Callback::new(move |()| submit())
                            />
                        </label>
                        <label class="form-field">
                            <span>"公司名称 "<em>"可选"</em></span>
                            <input
                                placeholder="输入公司名称"
                                prop:value=company_name
                                on:input=move |event| set_company_name.set(event_target_value(&event))
                                on:keydown=move |event| if event.key() == "Enter" { submit() }
                            />
                        </label>
                        <button
                            class="primary-button compact form-submit"
                            disabled=move || ticker.get().trim().is_empty() || mutate.pending().get()
                            on:click=move |_| submit()
                        >{move || if mutate.pending().get() { "正在添加…" } else { "添加关注" }}</button>
                    </div>
                    <div class="feedback-slot" aria-live="polite">
                        {move || mutate.value().get().and_then(Result::err).map(|error| view! {
                            <p class="inline-feedback is-error" role="alert">{error}</p>
                        })}
                    </div>
                </div>
                {move || match entries.get() {
                    None => loading_view(),
                    Some(Err(error)) => error_view(error),
                    Some(Ok(data)) => {
                        let visible: Vec<_> = data.entries.into_iter().filter(|entry| entry.mode == "add").collect();
                        if visible.is_empty() { empty_view("还没有自选公司。", "在上方填入代码或公司名即可加入自选。") } else { view! {
                            <div class="watch-table data-table">
                                <div class="watch-table-row data-table-head">
                                    <span>"公司"</span><span>"代码"</span><span>"标签 / 状态"</span><span>"最近关注"</span><span>"操作"</span>
                                </div>
                                {visible.into_iter().map(|entry| {
                                    let ticker_text = entry.ticker.clone();
                                    let company = entry.company_name.clone().unwrap_or_else(|| ticker_text.clone());
                                    let research_ticker = ticker_text.clone();
                                    let remove_ticker = ticker_text.clone();
                                    let market = market_label(&ticker_text);
                                    let initial = ticker_initial(&ticker_text);
                                    view! {
                                        <div class="watch-table-row">
                                            <span class="watch-company" data-label="公司">
                                                <i aria-hidden="true">{initial}</i><strong>{company}</strong>
                                            </span>
                                            <span class="watch-ticker" data-label="代码">{ticker_text.clone()}</span>
                                            <span class="watch-tags" data-label="标签 / 状态"><em>{market}</em><b>"长期跟踪"</b></span>
                                            <span class="watch-date" data-label="最近关注">{format::timestamp(&entry.created_at)}</span>
                                            <span class="watch-actions" data-label="操作">
                                                <button
                                                    class="watch-star"
                                                    title=format!("研究 {ticker_text}")
                                                    aria-label=format!("开始研究 {ticker_text}")
                                                    on:click=move |_| on_research.call(research_ticker.clone())
                                                ><Icon name="search" /></button>
                                                <button
                                                    class="watch-more danger-link"
                                                    title="移出关注"
                                                    aria-label=format!("移出关注 {remove_ticker}")
                                                    on:click=move |_| {
                                                        let target = remove_ticker.clone();
                                                        confirm_destructive(
                                                            "移出自选",
                                                            format!("{target} 将不再出现在自选与监控里；已创建的监控规则不会自动删除。"),
                                                            "移出自选",
                                                            Callback::new(move |_| mutate.dispatch(WatchAction {
                                                                track: false,
                                                                ticker: target.clone(),
                                                                company_name: None,
                                                            })),
                                                        );
                                                    }
                                                ><Icon name="trash" /></button>
                                            </span>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        }.into_view() }
                    },
                }}
            </section>
            <RulesDeskSection />
        </section>
    }
}

/// 规则种类的用户可读标签——列表里不给用户看 `price_below` 这类内部代码。
fn rule_kind_label(kind: &str) -> &str {
    match kind {
        "price_below" => "现价 ≤ 阈值",
        "price_above" => "现价 ≥ 阈值",
        "fundamental_below" => "基本面 ≤ 阈值",
        "fundamental_above" => "基本面 ≥ 阈值",
        "valuation_percentile_below" => "估值分位 ≤ 阈值",
        "valuation_percentile_above" => "估值分位 ≥ 阈值",
        "event_earnings" => "有新业绩事实",
        other => other,
    }
}

/// 监控规则 + 台面：新增/删除 `watch_rules`，聚合展示已跟踪 ticker 的行情、挂载的规则与
/// 近期触发通知——只读聚合，不新增写路径（写路径就是下面的新增/删除规则表单）。
#[component]
fn RulesDeskSection() -> impl IntoView {
    const KIND_OPTIONS: &[(&str, &str)] = &[
        ("price_below", "现价 ≤ 阈值"),
        ("price_above", "现价 ≥ 阈值"),
        ("fundamental_below", "基本面指标 ≤ 阈值"),
        ("fundamental_above", "基本面指标 ≥ 阈值"),
        (
            "valuation_percentile_below",
            "历史估值分位 ≤ 阈值（仅美股）",
        ),
        (
            "valuation_percentile_above",
            "历史估值分位 ≥ 阈值（仅美股）",
        ),
        ("event_earnings", "有新业绩事实（无需阈值）"),
    ];

    let (rule_ticker, set_rule_ticker) = create_signal(String::new());
    let (rule_kind, set_rule_kind) = create_signal("price_below".to_string());
    let (rule_threshold, set_rule_threshold) = create_signal(String::new());
    let (rule_metric, set_rule_metric) = create_signal(String::new());
    let (rule_label, set_rule_label) = create_signal(String::new());
    let (form_error, set_form_error) = create_signal(None::<String>);
    let (builder_open, set_builder_open) = create_signal(false);

    let desk = create_resource(|| (), |_| api::get::<DeskResponse>("/api/watch/desk"));
    // 规则表要能区分"这家公司还在自选里"和"公司已移出但规则还在跑"。`/api/watch/desk`
    // 会把任何挂了规则的 ticker 都列出来（见 watch_desk 的 names.entry(rule.ticker)），
    // 所以必须另取一次自选列表来判定，否则界面会让人以为这些公司仍在跟踪。
    let tracked = create_resource(|| (), |_| api::get::<WatchListResponse>("/api/watch/list"));
    let create_rule = create_action(|input: &WatchRuleCreateRequest| {
        let input = input.clone();
        async move { api::post::<_, echo_contracts::WatchRule>("/api/watch/rules", &input).await }
    });
    let delete_rule = create_action(|id: &i64| {
        let path = format!("/api/watch/rules?id={id}");
        async move { api::delete::<MutationResponse>(&path).await }
    });
    create_effect(move |_| {
        if matches!(create_rule.value().get(), Some(Ok(_))) {
            desk.refetch();
            set_rule_threshold.set(String::new());
            set_rule_metric.set(String::new());
            set_rule_label.set(String::new());
            set_builder_open.set(false);
        }
    });
    create_effect(move |_| {
        if matches!(delete_rule.value().get(), Some(Ok(_))) {
            desk.refetch();
        }
    });

    let submit = move || {
        let ticker = rule_ticker.get().trim().to_uppercase();
        if ticker.is_empty() {
            set_form_error.set(Some("请填写股票代码".into()));
            return;
        }
        let kind = rule_kind.get();
        let needs_threshold = kind != "event_earnings";
        let threshold = if needs_threshold {
            match rule_threshold.get().trim().parse::<Decimal>() {
                Ok(value) => Some(value),
                Err(_) => {
                    set_form_error.set(Some("阈值必须是有效数字".into()));
                    return;
                }
            }
        } else {
            None
        };
        let needs_metric = kind == "fundamental_below" || kind == "fundamental_above";
        if needs_metric && rule_metric.get().trim().is_empty() {
            set_form_error.set(Some("该规则种类需要填写基本面指标".into()));
            return;
        }
        set_form_error.set(None);
        create_rule.dispatch(WatchRuleCreateRequest {
            ticker,
            kind,
            threshold,
            metric: (!rule_metric.get().trim().is_empty()).then(|| rule_metric.get()),
            label: (!rule_label.get().trim().is_empty()).then(|| rule_label.get()),
        });
    };

    view! {
        <section class="rules-desk data-panel">
            <div class="panel-titlebar">
                <h2>"监控规则"</h2>
                <button
                    class=move || if builder_open.get() { "outline-button is-active" } else { "outline-button" }
                    aria-expanded=move || builder_open.get()
                    on:click=move |_| set_builder_open.update(|value| *value = !*value)
                >
                    <span aria-hidden="true">{move || if builder_open.get() { "−" } else { "+" }}</span>
                    {move || if builder_open.get() { "收起表单" } else { "新建规则" }}
                </button>
            </div>
            {move || builder_open.get().then(|| view! {
                <section class="action-panel rule-builder">
                    <div class="form-grid rule-form-grid">
                        <label class="form-field">
                            <span>"股票代码"</span>
                            <TickerSearchInput
                                value=rule_ticker
                                set_value=set_rule_ticker
                                placeholder="AAPL / 苹果"
                            />
                        </label>
                        <label class="form-field form-field-wide">
                            <span>"触发条件"</span>
                            <select on:change=move |event| set_rule_kind.set(event_target_value(&event))>
                                {KIND_OPTIONS.iter().map(|(value, label)| view! {
                                    <option value=*value selected=move || rule_kind.get() == *value>{*label}</option>
                                }).collect_view()}
                            </select>
                        </label>
                        <label class="form-field">
                            <span>"阈值"</span>
                            <input
                                placeholder=move || if rule_kind.get() == "event_earnings" { "无需填写" } else { "输入数值" }
                                disabled=move || rule_kind.get() == "event_earnings"
                                inputmode="decimal"
                                prop:value=rule_threshold
                                on:input=move |event| set_rule_threshold.set(event_target_value(&event))
                            />
                        </label>
                        <label class="form-field">
                            <span>"基本面指标 "<em>"按需"</em></span>
                            <input
                                placeholder="如 grossMargin"
                                disabled=move || !matches!(rule_kind.get().as_str(), "fundamental_below" | "fundamental_above")
                                prop:value=rule_metric
                                on:input=move |event| set_rule_metric.set(event_target_value(&event))
                            />
                        </label>
                        <label class="form-field form-field-wide">
                            <span>"说明 "<em>"可选"</em></span>
                            <input placeholder="例如：跌破长期观察位" prop:value=rule_label on:input=move |event| set_rule_label.set(event_target_value(&event)) />
                        </label>
                        <button class="primary-button compact form-submit" disabled=move || create_rule.pending().get() on:click=move |_| submit()>
                            {move || if create_rule.pending().get() { "正在创建…" } else { "创建规则" }}
                        </button>
                    </div>
                </section>
            })}
            <div class="feedback-slot" aria-live="polite">
                {move || form_error.get().map(|error| view! { <p class="inline-feedback is-error">{error}</p> })}
                {move || create_rule.value().get().map(|result| match result {
                    Ok(_) => view! { <p class="inline-feedback is-success">"规则已创建并开始监控"</p> }.into_view(),
                    Err(error) => view! { <p class="inline-feedback is-error">{error}</p> }.into_view(),
                })}
            </div>
            {move || match desk.get() {
                None => loading_view(),
                Some(Err(error)) => error_view(error),
                Some(Ok(data)) if data.tickers.is_empty() => empty_view(
                    "还没有跟踪任何公司。",
                    "先在「自选与监控」里关注一家公司。",
                ),
                Some(Ok(data)) => {
                    let rules: Vec<_> = data.tickers.into_iter().flat_map(|item| item.rules).collect();
                    if rules.is_empty() {
                        empty_view(
                            "已关注公司尚未设置监控规则。",
                            "用右上角的「新建规则」为它设定触发条件。",
                        )
                    } else {
                        let tracked_tickers: Vec<String> = tracked
                            .get()
                            .and_then(Result::ok)
                            .map(|data| data.entries.into_iter()
                                .filter(|entry| entry.mode == "add")
                                .map(|entry| entry.ticker)
                                .collect())
                            .unwrap_or_default();
                        view! {
                            <div class="rules-table data-table">
                                <div class="rules-table-row data-table-head">
                                    <span>"股票代码"</span><span>"触发条件"</span><span>"基本面指标"</span><span>"说明"</span><span>"操作"</span>
                                </div>
                                {rules.into_iter().map(|rule| {
                                    let rule_id = rule.id;
                                    let rule_ticker = rule.ticker.clone();
                                    let orphaned = !tracked_tickers.contains(&rule.ticker);
                                    let condition = if rule.kind == "event_earnings" {
                                        rule_kind_label(&rule.kind).to_string()
                                    } else {
                                        format!("{} {}", rule_kind_label(&rule.kind), rule.threshold)
                                    };
                                    let metric = rule.metric
                                        .as_deref()
                                        .map(|value| format::metric_label(value).to_string())
                                        .unwrap_or_else(|| "—".to_string());
                                    view! {
                                        <div class="rules-table-row">
                                            <span data-label="股票代码">
                                                <strong>{rule.ticker}</strong>
                                                // 移出自选不会自动删规则——规则还在后台跑并发通知，
                                                // 这里必须标出来，不能让人以为公司还在跟踪。
                                                {orphaned.then(|| view! {
                                                    <em class="rule-orphan" title="该公司已移出自选，但这条规则仍在后台运行">"已移出自选"</em>
                                                })}
                                            </span>
                                            <span data-label="触发条件">{condition}</span>
                                            <span data-label="基本面指标">{metric}</span>
                                            <span data-label="说明">{rule.label.unwrap_or_else(|| "持续监控".to_string())}</span>
                                            <span class="rules-actions" data-label="操作">
                                                <button
                                                    class="watch-more danger-link"
                                                    title="删除规则"
                                                    aria-label=format!("删除 {rule_ticker} 的监控规则")
                                                    on:click=move |_| confirm_destructive(
                                                        "删除监控规则",
                                                        "删除后这条规则不再触发通知，历史通知记录保留。",
                                                        "删除规则",
                                                        Callback::new(move |_| delete_rule.dispatch(rule_id)),
                                                    )
                                                ><Icon name="trash" /></button>
                                            </span>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        }.into_view()
                    }
                },
            }}
            <section class="activity-section">
                <div class="activity-head"><h3>"近期触发"</h3><span>"最近的规则命中记录"</span></div>
                <div class="activity-feed">
                    {move || match desk.get() {
                        None => loading_view(),
                        Some(Err(error)) => error_view(error),
                        Some(Ok(data)) if data.recent_triggers.is_empty() => empty_view(
                            "暂无触发记录。",
                            "监控规则命中条件时，这里会留下时间与当时的数值。",
                        ),
                        Some(Ok(data)) => data.recent_triggers.into_iter().map(|item| view! {
                            <article class="notice">
                                <i aria-hidden="true"></i>
                                <div>
                                    <strong>{item.title}</strong>
                                    <p>{item.body}</p>
                                    <time>{format::timestamp(&item.created_at)}</time>
                                </div>
                            </article>
                        }).collect_view().into_view(),
                    }}
                </div>
            </section>
        </section>
    }
}

#[component]
fn PortfolioSection() -> impl IntoView {
    let (ticker, set_ticker) = create_signal(String::new());
    let (company_name, set_company_name) = create_signal(String::new());
    let (shares, set_shares) = create_signal(String::new());
    let (avg_cost, set_avg_cost) = create_signal(String::new());
    let (stop_loss, set_stop_loss) = create_signal(String::new());
    let (take_profit, set_take_profit) = create_signal(String::new());
    let (form_error, set_form_error) = create_signal(None::<String>);
    let positions = create_resource(
        || (),
        |_| api::get::<PortfolioListResponse>("/api/portfolio"),
    );
    let save = create_action(|input: &PortfolioUpsertRequest| {
        let input = input.clone();
        async move { api::post::<_, echo_contracts::PortfolioPosition>("/api/portfolio", &input).await }
    });
    let remove = create_action(|ticker: &String| {
        let path = format!("/api/portfolio?ticker={ticker}");
        async move { api::delete::<MutationResponse>(&path).await }
    });
    create_effect(move |_| {
        if matches!(save.value().get(), Some(Ok(_))) || matches!(remove.value().get(), Some(Ok(_)))
        {
            positions.refetch();
        }
        if matches!(save.value().get(), Some(Ok(_))) {
            set_ticker.set(String::new());
            set_company_name.set(String::new());
            set_shares.set(String::new());
            set_avg_cost.set(String::new());
            set_stop_loss.set(String::new());
            set_take_profit.set(String::new());
        }
    });
    let submit = move || {
        if ticker.get().trim().is_empty() {
            set_form_error.set(Some("请先填写股票代码".into()));
            return;
        }
        let parsed_shares = shares.get().parse::<Decimal>();
        let parsed_cost = avg_cost.get().parse::<Decimal>();
        // 止损/止盈可选——留空即不设，但填了就必须是有效数字，不能静默丢弃。
        let parse_optional = |text: String| -> Result<Option<Decimal>, ()> {
            let trimmed = text.trim().to_string();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                trimmed.parse::<Decimal>().map(Some).map_err(|_| ())
            }
        };
        let parsed_stop = parse_optional(stop_loss.get());
        let parsed_take = parse_optional(take_profit.get());
        match (parsed_shares, parsed_cost, parsed_stop, parsed_take) {
            (Ok(shares), Ok(avg_cost), Ok(stop_loss), Ok(take_profit)) => {
                set_form_error.set(None);
                save.dispatch(PortfolioUpsertRequest {
                    ticker: ticker.get().trim().to_uppercase(),
                    company_name: (!company_name.get().trim().is_empty())
                        .then(|| company_name.get()),
                    shares,
                    avg_cost,
                    stop_loss,
                    take_profit,
                    note: String::new(),
                });
            }
            _ => set_form_error.set(Some("股数、成本与风控线必须是有效数字".into())),
        }
    };
    view! {
        <section class="library-section">
            <div class="section-heading">
                <div>
                    <h2>"持仓与风控"</h2>
                    <p>"记录真实仓位，并把止损与止盈线放到同一个决策视图。"</p>
                </div>
                <span class="section-status is-neutral">"仅你可见"</span>
            </div>
            <section class="action-panel portfolio-form">
                <div class="form-grid portfolio-form-grid">
                    <label class="form-field">
                        <span>"股票代码"</span>
                        <TickerSearchInput
                            value=ticker
                            set_value=set_ticker
                            placeholder="AAPL / 苹果"
                            on_pick=Callback::new(move |item: CompanySearchItem| {
                                if company_name.get_untracked().trim().is_empty() {
                                    set_company_name.set(item.name_zh);
                                }
                            })
                        />
                    </label>
                    <label class="form-field form-field-wide">
                        <span>"公司名称"</span>
                        <input placeholder="Apple Inc." prop:value=company_name on:input=move |event| set_company_name.set(event_target_value(&event)) />
                    </label>
                    <label class="form-field">
                        <span>"持有股数"</span>
                        <input placeholder="0" inputmode="decimal" prop:value=shares on:input=move |event| set_shares.set(event_target_value(&event)) />
                    </label>
                    <label class="form-field">
                        <span>"平均成本"</span>
                        <input placeholder="0.00" inputmode="decimal" prop:value=avg_cost on:input=move |event| set_avg_cost.set(event_target_value(&event)) />
                    </label>
                    <label class="form-field">
                        <span>"止损线 "<em>"可选"</em></span>
                        <input placeholder="0.00" inputmode="decimal" prop:value=stop_loss on:input=move |event| set_stop_loss.set(event_target_value(&event)) />
                    </label>
                    <label class="form-field">
                        <span>"止盈线 "<em>"可选"</em></span>
                        <input placeholder="0.00" inputmode="decimal" prop:value=take_profit on:input=move |event| set_take_profit.set(event_target_value(&event)) />
                    </label>
                    <button class="primary-button compact form-submit" disabled=move || save.pending().get() on:click=move |_| submit()>
                        {move || if save.pending().get() { "正在保存…" } else { "保存持仓" }}
                    </button>
                </div>
            </section>
            <div class="feedback-slot" aria-live="polite">
                {move || form_error.get().map(|error| view! { <p class="inline-feedback is-error">{error}</p> })}
                {move || save.value().get().map(|result| match result {
                    Ok(_) => view! { <p class="inline-feedback is-success">"持仓已保存"</p> }.into_view(),
                    Err(error) => view! { <p class="inline-feedback is-error">{error}</p> }.into_view(),
                })}
            </div>
            {move || match positions.get() {
                None => loading_view(),
                Some(Err(error)) => error_view(error),
                Some(Ok(data)) if data.positions.is_empty() => empty_view("还没有持仓。", "在上方录入成本与股数后，这里会算出盈亏。"),
                Some(Ok(data)) => view! { <div class="portfolio-table">
                    <div class="table-row table-head"><span>"公司"</span><span>"股数"</span><span>"平均成本"</span><span>"风控线"</span><span></span></div>
                    {data.positions.into_iter().map(|position| {
                        let remove_ticker = position.ticker.clone();
                        let label = position.company_name.clone();
                        view! { <div class="table-row">
                            <span data-label="公司"><strong>{position.company_name}</strong><small>{position.ticker}</small></span>
                            <span data-label="股数">{decimal_text(position.shares)}</span>
                            <span data-label="平均成本">{decimal_text(position.avg_cost)}</span>
                            <span data-label="风控线">{format!("{} / {}", decimal_text(position.stop_loss), decimal_text(position.take_profit))}</span>
                            <button
                                class="danger-link"
                                aria-label=format!("删除 {remove_ticker} 的持仓记录")
                                on:click=move |_| {
                                    let target = remove_ticker.clone();
                                    confirm_destructive(
                                        "删除持仓记录",
                                        format!("{label} 的股数、成本与风控线将一并删除，无法恢复。"),
                                        "删除持仓",
                                        Callback::new(move |_| remove.dispatch(target.clone())),
                                    );
                                }
                            ><Icon name="trash" />"删除"</button>
                        </div> }
                    }).collect_view()}
                </div> }.into_view(),
            }}
        </section>
    }
}

/// 免打扰时段的默认建议值——用户此前没设过时，输入框给一个可直接保存的合理起点，
/// 而不是显示空的 `--:--` 让用户猜格式。
const QUIET_START_DEFAULT: &str = "22:30";
const QUIET_END_DEFAULT: &str = "07:00";

#[component]
fn SettingsPage(role: UserRole) -> impl IntoView {
    let (initialized, set_initialized) = create_signal(false);
    let (notify_digest, set_notify_digest) = create_signal(true);
    let (notify_positions, set_notify_positions) = create_signal(true);
    let (notify_falsify, set_notify_falsify) = create_signal(true);
    let (notify_review, set_notify_review) = create_signal(true);
    let (notify_earnings, set_notify_earnings) = create_signal(true);
    let (quiet_enabled, set_quiet_enabled) = create_signal(false);
    let (quiet_start, set_quiet_start) = create_signal(QUIET_START_DEFAULT.to_string());
    let (quiet_end, set_quiet_end) = create_signal(QUIET_END_DEFAULT.to_string());
    let preferences = create_resource(
        || (),
        |_| api::get::<PreferencesResponse>("/api/preferences"),
    );
    create_effect(move |_| {
        if initialized.get_untracked() {
            return;
        }
        if let Some(Ok(data)) = preferences.get() {
            let value = data.preferences;
            set_notify_digest.set(value.notify_digest);
            set_notify_positions.set(value.notify_positions);
            set_notify_falsify.set(value.notify_falsify);
            set_notify_review.set(value.notify_review);
            set_notify_earnings.set(value.notify_earnings);
            // 服务端用空串表示"未设置免打扰"；界面用开关显式表达这个状态，
            // 而不是把空值塞进 time 输入框显示成 `--:--`。
            let start = value.quiet_hours_start.unwrap_or_default();
            let end = value.quiet_hours_end.unwrap_or_default();
            let configured = !start.is_empty() && !end.is_empty();
            set_quiet_enabled.set(configured);
            if configured {
                set_quiet_start.set(start);
                set_quiet_end.set(end);
            }
            set_initialized.set(true);
        }
    });
    let save = create_action(|input: &PreferencesUpdateRequest| {
        let input = input.clone();
        async move { api::patch::<_, PreferencesResponse>("/api/preferences", &input).await }
    });
    let dispatch_save = move || {
        // 关掉免打扰就明确写空串清掉，不留下一段用户以为已经关掉、实际还在生效的时段。
        let (start, end) = if quiet_enabled.get() {
            (quiet_start.get(), quiet_end.get())
        } else {
            (String::new(), String::new())
        };
        save.dispatch(PreferencesUpdateRequest {
            onboarding_completed: None,
            notify_digest: Some(notify_digest.get()),
            notify_positions: Some(notify_positions.get()),
            notify_falsify: Some(notify_falsify.get()),
            notify_review: Some(notify_review.get()),
            notify_earnings: Some(notify_earnings.get()),
            quiet_hours_start: Some(start),
            quiet_hours_end: Some(end),
        });
    };
    view! {
        <PageHeader title="设置" detail="通知与免打扰。开关在通知写入咽喉生效，后台作业无法绕过。" />
        <main class="page-content settings-page-content">
            // 页头已经说清这页是干什么的；再放一条"通知由你掌控 / 保存后立即生效"的
            // 横幅只是把同一句话说第二遍。
            <div class="settings-grid">
                <section class="settings-card">
                    <div class="settings-card-head"><h2>"外观"</h2></div>
                    <p class="muted">"跟随系统会随你的操作系统在日夜之间切换。"</p>
                    {use_context::<RwSignal<crate::theme::Theme>>()
                        .map(|theme| view! { <crate::theme::ThemePicker theme=theme /> })}
                </section>
                <section class="settings-card settings-delivery-card">
                    <div class="settings-card-head"><h2>"通知类型"</h2></div>
                    <Toggle icon="clock" label="盘前 / 盘后摘要" detail="每天汇总市场与自选公司变化" value=notify_digest set_value=set_notify_digest />
                    <Toggle icon="bell" label="仓位价格告警" detail="持仓触及止损或止盈线时提醒" value=notify_positions set_value=set_notify_positions />
                    <Toggle icon="pulse" label="证伪条件触发" detail="核心论点的证伪条件出现时提醒" value=notify_falsify set_value=set_notify_falsify />
                    <Toggle icon="calendar" label="定期研究复盘" detail="按周期提醒重新审视研究结论" value=notify_review set_value=set_notify_review />
                    <Toggle icon="chart" label="业绩跟踪更新" detail="财报发布后更新事实与判断" value=notify_earnings set_value=set_notify_earnings />
                </section>
                <section class="settings-card quiet-hours-card">
                    <div class="settings-card-head"><h2>"免打扰时段"</h2></div>
                    <p class="muted">"普通通知在该时段暂停；证伪和仓位告警仍会送达。"</p>
                    <Toggle
                        icon="clock"
                        label="启用免打扰"
                        detail="关闭时不设置任何静音时段"
                        value=quiet_enabled
                        set_value=set_quiet_enabled
                    />
                    {move || quiet_enabled.get().then(|| view! {
                        <div class="time-range">
                            <label>
                                <span>"开始"</span>
                                <div class="time-input-shell">
                                    <SettingsGlyph kind="clock" />
                                    <input type="time" prop:value=quiet_start on:input=move |event| set_quiet_start.set(event_target_value(&event)) />
                                </div>
                            </label>
                            <span class="time-range-divider" aria-hidden="true">"—"</span>
                            <label>
                                <span>"结束"</span>
                                <div class="time-input-shell">
                                    <SettingsGlyph kind="clock" />
                                    <input type="time" prop:value=quiet_end on:input=move |event| set_quiet_end.set(event_target_value(&event)) />
                                </div>
                            </label>
                        </div>
                    })}
                    <div class="quiet-note"><span aria-hidden="true">"i"</span>"紧急风控提醒不会被静音"</div>
                </section>
                {(role == UserRole::Owner).then(|| view! { <InviteCard /> })}
            </div>
            <div class="settings-actions">
                <div aria-live="polite">
                    {move || preferences.get().and_then(Result::err).map(|error| view! { <p class="inline-feedback is-error">{error}</p> })}
                    {move || save.value().get().map(|result| match result {
                        Ok(_) => view! { <p class="inline-feedback is-success">"设置已保存"</p> }.into_view(),
                        Err(error) => view! { <p class="inline-feedback is-error">{error}</p> }.into_view(),
                    })}
                </div>
                <button
                    class="primary-button settings-save"
                    aria-busy=move || save.pending().get()
                    disabled=move || save.pending().get()
                    on:click=move |_| dispatch_save()
                >{move || if save.pending().get() { "正在保存…" } else { "保存设置" }}</button>
            </div>
        </main>
    }
}

/// Owner 专属：生成邀请码。
///
/// 注册表单一直强制要求邀请码，而生成邀请码的 `/api/auth/invite` 在界面上没有任何入口——
/// 结果是 owner 只能靠手敲 HTTP 请求才能把人拉进来，等于这个产品实际上无法邀请第二个用户。
#[component]
fn InviteCard() -> impl IntoView {
    let (copied, set_copied) = create_signal(false);
    let create = create_action(|(): &()| async move {
        api::post::<_, AuthInviteResponse>("/api/auth/invite", &AuthInviteRequest { note: None })
            .await
    });
    let latest = move || {
        create
            .value()
            .get()
            .and_then(Result::ok)
            .map(|res| res.code)
    };
    view! {
        <section class="settings-card">
            <div class="settings-card-head"><h2>"邀请成员"</h2></div>
            <p class="muted">"注册需要邀请码。生成后交给对方，一码一人，用过即失效。"</p>
            <button
                class="primary-button compact"
                aria-busy=move || create.pending().get()
                disabled=move || create.pending().get()
                on:click=move |_| {
                    set_copied.set(false);
                    create.dispatch(());
                }
            >{move || if create.pending().get() { "正在生成…" } else { "生成邀请码" }}</button>
            <div aria-live="polite">
                {move || latest().map(|code| {
                    let to_copy = code.clone();
                    view! {
                        <div class="invite-code">
                            // 邀请码要能一眼读准确：等宽 + 可选中，别让用户抄错一个 O 和 0。
                            <code>{code}</code>
                            <button
                                class="answer-action"
                                on:click=move |_| {
                                    api::copy_text(&to_copy);
                                    set_copied.set(true);
                                }
                            >{move || if copied.get() { "已复制" } else { "复制" }}</button>
                        </div>
                    }
                })}
                {move || create.value().get().and_then(Result::err).map(|error| view! {
                    <p class="inline-feedback is-error" role="alert">{error}</p>
                })}
            </div>
        </section>
    }
}

#[component]
fn SettingsGlyph(kind: &'static str) -> impl IntoView {
    let drawing = match kind {
        "check" => view! {
            <circle cx="12" cy="12" r="7.5"></circle>
            <path d="m8.7 12.1 2.1 2.1 4.6-5"></path>
        }
        .into_view(),
        "bell" => view! {
            <path d="M7.5 10.2a4.5 4.5 0 0 1 9 0c0 3 .9 4.1 1.5 4.8H6c.6-.7 1.5-1.8 1.5-4.8Z"></path>
            <path d="M10.2 18a2 2 0 0 0 3.6 0"></path>
        }
        .into_view(),
        "pulse" => view! {
            <path d="M3.5 12h4l1.7-5.5 3.2 11 2-7 1.3 1.5h4.8"></path>
        }
        .into_view(),
        "calendar" => view! {
            <rect x="5" y="6.5" width="14" height="13" rx="2"></rect>
            <path d="M8 4.5v4M16 4.5v4M5 10.5h14M9 14h2M13 14h2"></path>
        }
        .into_view(),
        "chart" => view! {
            <path d="M4.5 19.5h15"></path>
            <rect x="6" y="11" width="2.6" height="6.5" rx=".5"></rect>
            <rect x="10.7" y="6.5" width="2.6" height="11" rx=".5"></rect>
            <rect x="15.4" y="9" width="2.6" height="8.5" rx=".5"></rect>
        }
        .into_view(),
        _ => view! {
            <circle cx="12" cy="12" r="7.5"></circle>
            <path d="M12 8v4l2.5 1.5"></path>
        }
        .into_view(),
    };
    view! {
        <span class=format!("settings-glyph is-{kind}") aria-hidden="true">
            <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">{drawing}</svg>
        </span>
    }
}

#[component]
fn Toggle(
    icon: &'static str,
    label: &'static str,
    detail: &'static str,
    value: ReadSignal<bool>,
    set_value: WriteSignal<bool>,
) -> impl IntoView {
    view! {
        <label class="toggle-row">
            <SettingsGlyph kind=icon />
            <span class="toggle-copy"><strong>{label}</strong><small>{detail}</small></span>
            <input type="checkbox" role="switch" aria-label=label prop:checked=value on:change=move |event| set_value.set(event_target_checked(&event)) />
        </label>
    }
}

#[component]
fn PageHeader(title: &'static str, detail: &'static str) -> impl IntoView {
    view! {
        // 不放 "ECHO WORKSPACE" 之类的英文小标签——顶栏左上角已经写着 ECHO，
        // 每页再复述一遍只是占掉标题上方的一行。
        <header class="page-header">
            <h1>{title}</h1>
            <p>{detail}</p>
        </header>
    }
}

fn decimal_text(value: Option<Decimal>) -> String {
    value
        .map(|value| value.normalize().to_string())
        .unwrap_or_else(|| "—".into())
}

pub(crate) fn loading_view() -> View {
    view! {
        <div class="page-state is-loading" aria-live="polite">
            <span class="state-loader" aria-hidden="true"><i></i><i></i><i></i></span>
            <p><strong>"正在读取"</strong><small>"正在同步最新数据…"</small></p>
        </div>
    }
    .into_view()
}

pub(crate) fn error_view(error: String) -> View {
    view! {
        <div class="page-state is-error" role="alert">
            <span class="state-symbol" aria-hidden="true">"!"</span>
            <p><strong>"暂时无法读取"</strong><small>{error}</small></p>
        </div>
    }
    .into_view()
}

/// 空态。`hint` 说清"这里的内容从哪来"。
///
/// 原来七处空态共用同一句「完成上方操作后，内容会出现在这里。」——对自选/持仓/规则
/// 是对的（上面确实有表单），但通知和触发记录是系统跑出来的，上方没有任何操作可做，
/// 那句话是在指使用户去点一个不存在的东西。空态是产品少数几次主动说话的机会，
/// 说错方向比不说更糟。
pub(crate) fn empty_view(message: &'static str, hint: &'static str) -> View {
    view! {
        <div class="page-state is-empty">
            <span class="state-symbol" aria-hidden="true">"·"</span>
            <p><strong>{message}</strong><small>{hint}</small></p>
        </div>
    }
    .into_view()
}
