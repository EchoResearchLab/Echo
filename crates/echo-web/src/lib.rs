//! Echo Research 的纯 Rust 浏览器应用：Leptos 组件编译为 WASM，所有请求/响应与服务端
//! 共用 `echo-contracts`，页面与 API 模型由 Rust 类型共同约束。

mod api;
mod dialog;
mod format;
mod icons;
mod markdown;
mod profiles;
mod research;
mod theme;
mod workspace;

use dialog::{ConfirmHost, provide_confirm};
use echo_contracts::AuthMeResponse;
use leptos::*;
use workspace::{LoginPage, Workspace};

#[component]
pub fn App() -> impl IntoView {
    // 破坏性操作的确认通道在根注入，任何层级的组件都能发起，宿主只有一个。
    provide_confirm();
    // 外观在根注入并立即应用：晚一帧应用会让深色用户先闪一下白底。
    let theme = create_rw_signal(theme::load());
    theme::apply(theme.get_untracked());
    provide_context(theme);
    let (auth_epoch, set_auth_epoch) = create_signal(0u64);
    let auth = create_resource(
        move || auth_epoch.get(),
        |_| api::get::<AuthMeResponse>("/api/auth/me"),
    );
    let refresh_auth = Callback::new(move |_| set_auth_epoch.update(|value| *value += 1));
    view! {
        <Suspense fallback=move || view! { <main class="boot-screen">"ECHO"</main> }>
            {move || match auth.get() {
                None => ().into_view(),
                Some(Ok(response)) => match response.user {
                    Some(user) => view! { <Workspace user=user on_auth_changed=refresh_auth /> }.into_view(),
                    None => view! { <LoginPage on_authenticated=refresh_auth /> }.into_view(),
                },
                Some(Err(_)) => view! { <LoginPage on_authenticated=refresh_auth /> }.into_view(),
            }}
        </Suspense>
        <ConfirmHost />
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    _ = std::panic::take_hook();
    std::panic::set_hook(Box::new(|info| {
        leptos::logging::error!("echo-web panic: {info}");
    }));
    leptos::mount_to_body(App);
}
