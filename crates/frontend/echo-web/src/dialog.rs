//! 破坏性操作的统一确认对话框。
//!
//! 之前删除自选/持仓/规则/研究记录走的是浏览器原生 `window.confirm`——样式不可控、文案不能
//! 分级、在移动端表现为系统弹窗，和产品其余部分完全脱节。这里改成一个应用内对话框：
//! 任何组件通过 context 里的 [`ConfirmBus`] 发起请求，宿主只有一个，焦点与 Escape 统一处理。

use leptos::*;

/// 一次待确认的破坏性操作。
#[derive(Clone)]
pub struct ConfirmRequest {
    pub title: String,
    pub body: String,
    /// 确认按钮文案——描述结果（"删除记录"），不用含糊的"确定"。
    pub confirm_label: String,
    pub on_confirm: Callback<()>,
}

/// 全局确认通道。`Copy` 让它能被随意捕获进闭包。
#[derive(Clone, Copy)]
pub struct ConfirmBus(RwSignal<Option<ConfirmRequest>>);

impl ConfirmBus {
    pub fn ask(&self, request: ConfirmRequest) {
        self.0.set(Some(request));
    }
}

/// 在应用根注入确认通道；与 [`ConfirmHost`] 配对使用。
pub fn provide_confirm() -> ConfirmBus {
    let bus = ConfirmBus(create_rw_signal(None));
    provide_context(bus);
    bus
}

/// 取出确认通道。根组件已注入，缺失即为接线错误，直接 panic 比静默失效好。
pub fn use_confirm() -> ConfirmBus {
    expect_context::<ConfirmBus>()
}

/// 便捷封装：一行发起一次确认请求。
pub fn confirm_destructive(
    title: impl Into<String>,
    body: impl Into<String>,
    confirm_label: impl Into<String>,
    on_confirm: Callback<()>,
) {
    use_confirm().ask(ConfirmRequest {
        title: title.into(),
        body: body.into(),
        confirm_label: confirm_label.into(),
        on_confirm,
    });
}

/// 对话框宿主——挂在应用根，同一时刻最多一个对话框。
#[component]
pub fn ConfirmHost() -> impl IntoView {
    let bus = use_confirm();
    let pending = bus.0;
    install_escape_listener(pending);
    view! {
        {move || pending.get().map(|request| {
            let on_confirm = request.on_confirm;
            view! {
                <div class="dialog-scrim" role="presentation" on:click=move |_| pending.set(None)></div>
                <div class="dialog-shell" role="dialog" aria-modal="true" aria-labelledby="confirm-dialog-title">
                    <h2 id="confirm-dialog-title">{request.title.clone()}</h2>
                    <p>{request.body.clone()}</p>
                    <div class="dialog-actions">
                        <button class="outline-button" on:click=move |_| pending.set(None)>"取消"</button>
                        <button
                            class="primary-button compact is-danger"
                            on:click=move |_| {
                                pending.set(None);
                                on_confirm.call(());
                            }
                        >{request.confirm_label.clone()}</button>
                    </div>
                </div>
            }
        })}
    }
}

/// Escape 关闭对话框——键盘用户不该被一个只能点鼠标的弹窗困住。
#[cfg(target_arch = "wasm32")]
fn install_escape_listener(pending: RwSignal<Option<ConfirmRequest>>) {
    let handle = window_event_listener(ev::keydown, move |event| {
        if event.key() == "Escape" && pending.get_untracked().is_some() {
            pending.set(None);
        }
    });
    on_cleanup(move || handle.remove());
}

#[cfg(not(target_arch = "wasm32"))]
fn install_escape_listener(_pending: RwSignal<Option<ConfirmRequest>>) {}
