//! 外观主题——系统跟随 / 浅色 / 深色。
//!
//! 只做一件事：把选择写到 `<html data-theme>` 上并持久化。所有配色都在
//! `01-tokens.css` 的令牌层解决，组件样式里不该出现任何深色分支。
//!
//! "跟随系统"用的是**移除属性**而不是写入某个值：令牌层的 `prefers-color-scheme`
//! 媒体查询在没有 `data-theme` 时生效，写死一个值反而会让系统切换失灵。

use leptos::*;

// 这三处只在 wasm 目标的读写路径与单测里用到；原生目标编译 lib 时没有调用者，
// 但它们不是死代码——去掉条件 allow 会让 `clippy --all-targets` 与实际用途打架。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const STORAGE_KEY: &str = "echo.theme";

/// 用户的外观选择。`System` 是默认值，也是没存过任何选择时的状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    /// 存储值 / `data-theme` 属性值。`System` 没有属性值——它靠移除属性表达。
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    fn as_attr(self) -> Option<&'static str> {
        match self {
            Self::System => None,
            Self::Light => Some("light"),
            Self::Dark => Some("dark"),
        }
    }

    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    fn from_storage(value: &str) -> Self {
        match value {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "跟随系统",
            Self::Light => "浅色",
            Self::Dark => "深色",
        }
    }

    #[must_use]
    pub fn all() -> [Self; 3] {
        [Self::System, Self::Light, Self::Dark]
    }
}

#[cfg(target_arch = "wasm32")]
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// 读回上次的选择；读不到（首次访问 / 隐私模式禁用了 storage）就跟随系统。
#[must_use]
pub fn load() -> Theme {
    #[cfg(target_arch = "wasm32")]
    {
        storage()
            .and_then(|s| s.get_item(STORAGE_KEY).ok().flatten())
            .map(|v| Theme::from_storage(v.as_str()))
            .unwrap_or_default()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Theme::default()
    }
}

/// 应用到 `<html>` 并持久化。storage 写失败不影响本次生效——宁可这次能用、下次忘掉，
/// 也不要因为写不进去就不切换。
pub fn apply(theme: Theme) {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(root) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
        else {
            return;
        };
        match theme.as_attr() {
            Some(value) => {
                let _ = root.set_attribute("data-theme", value);
            }
            None => {
                let _ = root.remove_attribute("data-theme");
            }
        }
        if let Some(s) = storage() {
            let value = theme.as_attr().unwrap_or("system");
            let _ = s.set_item(STORAGE_KEY, value);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = theme;
    }
}

/// 外观选择的分段控件。与登录页的登录/注册切换共用同一套 `.auth-tabs` 语言。
#[component]
pub fn ThemePicker(theme: RwSignal<Theme>) -> impl IntoView {
    view! {
        <div class="auth-tabs theme-picker" role="group" aria-label="外观">
            {Theme::all().into_iter().map(|option| {
                view! {
                    <button
                        type="button"
                        class:is-active=move || theme.get() == option
                        aria-pressed=move || (theme.get() == option).to_string()
                        on:click=move |_| {
                            theme.set(option);
                            apply(option);
                        }
                    >{option.label()}</button>
                }
            }).collect_view()}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_has_no_attribute_so_media_query_stays_in_control() {
        assert_eq!(Theme::System.as_attr(), None);
        assert_eq!(Theme::Light.as_attr(), Some("light"));
        assert_eq!(Theme::Dark.as_attr(), Some("dark"));
    }

    #[test]
    fn unknown_storage_values_fall_back_to_system() {
        assert_eq!(Theme::from_storage("dark"), Theme::Dark);
        assert_eq!(Theme::from_storage("light"), Theme::Light);
        assert_eq!(Theme::from_storage(""), Theme::System);
        assert_eq!(Theme::from_storage("solarized"), Theme::System);
    }
}
