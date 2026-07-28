//! 代码输入框的公司搜索补全。
//!
//! 服务端的 `/api/companies/search` 一直存在却没有任何前端调用方：用户想加一只港股，
//! 得自己记住 "0700.HK" 这种带后缀的精确写法，打错一个字符就是一条查不到的自选。
//! 这里把它接回输入框——输代码或中文名都能搜，选中时把代码与公司名一起回填。
//!
//! 只补全**库里已建档**的公司。搜不到不代表不能研究（研究链路自己会解析并建档），
//! 所以搜不到时静默，不显示"无结果"来劝退用户手动输入。

use crate::api;
use echo_contracts::{CompanySearchItem, CompanySearchResponse};
use leptos::*;

/// 起搜的最短输入。1 个字符能匹配上库里几乎所有公司，翻页式的噪声反而挡住输入框。
const MIN_QUERY_CHARS: usize = 2;
/// 停止输入多久后才真正发请求。逐键请求会让"0700.HK"打出 7 个并发查询，
/// 而用户根本来不及看清中间任何一版结果。
#[cfg(target_arch = "wasm32")]
const DEBOUNCE_MS: i32 = 220;

#[cfg(target_arch = "wasm32")]
fn after_debounce(callback: impl FnOnce() + 'static) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;

    let closure = Closure::once(callback);
    let _ = leptos::window().set_timeout_with_callback_and_timeout_and_arguments_0(
        closure.as_ref().unchecked_ref(),
        DEBOUNCE_MS,
    );
    closure.forget();
}

#[cfg(not(target_arch = "wasm32"))]
fn after_debounce(callback: impl FnOnce() + 'static) {
    callback();
}

/// 带补全的股票代码输入框。
///
/// `value`/`set_value` 由调用方持有——补全只是输入手段之一，手打依然完全可用。
/// `on_pick` 在选中某条建议时触发，让调用方顺手回填公司名之类的关联字段。
#[component]
pub fn TickerSearchInput(
    value: ReadSignal<String>,
    set_value: WriteSignal<String>,
    #[prop(into)] placeholder: String,
    #[prop(optional)] on_pick: Option<Callback<CompanySearchItem>>,
    #[prop(optional)] on_submit: Option<Callback<()>>,
) -> impl IntoView {
    // 与输入框分离的查询信号：只有防抖窗口结束后才追上输入值，resource 因此不会逐键重跑。
    let (query, set_query) = create_signal(String::new());
    let (open, set_open) = create_signal(false);

    let suggestions = create_resource(
        move || query.get(),
        |q| async move {
            let trimmed = q.trim().to_string();
            if trimmed.chars().count() < MIN_QUERY_CHARS {
                return Vec::new();
            }
            api::get::<CompanySearchResponse>(&format!(
                "/api/companies/search?q={}&limit=8",
                encode_query(&trimmed)
            ))
            .await
            .map(|data| data.companies)
            // 搜不到、没登录、后端没配库——一律当作"没有建议"。补全失败绝不能挡住手动输入。
            .unwrap_or_default()
        },
    );

    let sync_query = move || {
        let typed = value.get_untracked();
        after_debounce(move || {
            // 防抖窗口内又敲了新字符就作废这一次：只有最后一次输入值配得上发请求。
            if value.get_untracked() == typed {
                set_query.set(typed);
            }
        });
    };

    let pick = move |item: CompanySearchItem| {
        set_value.set(item.ticker.clone());
        set_open.set(false);
        set_query.set(String::new());
        if let Some(callback) = on_pick {
            callback.call(item);
        }
    };

    view! {
        <div class="ticker-search">
            <input
                class="ticker-search-input"
                placeholder=placeholder
                autocomplete="off"
                role="combobox"
                aria-autocomplete="list"
                aria-expanded=move || if open.get() { "true" } else { "false" }
                prop:value=value
                on:input=move |event| {
                    // 代码统一大写，中文名保持原样——大写化只对 ASCII 生效。
                    set_value.set(event_target_value(&event).to_uppercase());
                    set_open.set(true);
                    sync_query();
                }
                on:focus=move |_| {
                    set_open.set(true);
                    sync_query();
                }
                // 失焦立刻收起会让点击建议变成"点了个空气"（blur 先于 click）。
                on:blur=move |_| after_debounce(move || set_open.set(false))
                on:keydown=move |event| match event.key().as_str() {
                    "Escape" => set_open.set(false),
                    "Enter" => {
                        set_open.set(false);
                        if let Some(callback) = on_submit {
                            callback.call(());
                        }
                    }
                    _ => {}
                }
            />
            {move || {
                let items = suggestions.get().unwrap_or_default();
                (open.get() && !items.is_empty()).then(|| view! {
                    <ul class="ticker-search-menu" role="listbox">
                        {items.into_iter().map(|item| {
                            let picked = item.clone();
                            let secondary = item
                                .name_en
                                .clone()
                                .filter(|name| !name.trim().is_empty())
                                .or_else(|| item.sector.clone())
                                .unwrap_or_default();
                            view! {
                                <li role="option">
                                    <button
                                        type="button"
                                        class="ticker-search-option"
                                        // mousedown 早于 blur：用 click 的话菜单已经收起了。
                                        on:mousedown=move |event| {
                                            event.prevent_default();
                                            pick(picked.clone());
                                        }
                                    >
                                        <strong>{item.ticker.clone()}</strong>
                                        <span>{item.name_zh.clone()}</span>
                                        {(!secondary.is_empty()).then(|| view! {
                                            <small>{secondary}</small>
                                        })}
                                    </button>
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                })
            }}
        </div>
    }
}

/// 极简 URL 查询编码。搜索词可能带空格、`&` 或中文，直接拼进 query string 会截断请求。
fn encode_query(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::encode_query;

    #[test]
    fn ticker_passes_through_and_unsafe_bytes_are_escaped() {
        assert_eq!(encode_query("0700.HK"), "0700.HK");
        assert_eq!(encode_query("a b&c"), "a%20b%26c");
    }

    #[test]
    fn chinese_names_are_percent_encoded_per_utf8_byte() {
        assert_eq!(encode_query("腾讯"), "%E8%85%BE%E8%AE%AF");
    }
}
