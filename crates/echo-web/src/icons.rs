//! 统一图标集。
//!
//! 之前界面里混用了 `×`/`⋮`/`★`/`↑`/`显示` 等字符与中文当按钮内容——字形随系统字体漂移、
//! 尺寸不可控，`⋮`（"更多"）还被拿去当删除，语义是错的。这里用一套 20×20 线性图标统一收口：
//! 描边取 `currentColor`，尺寸由 CSS 决定，语义由调用处的 `aria-label` 承担。

use leptos::*;

/// 图标名到路径数据的唯一映射。新增图标只在这里加一条。
fn path_for(name: &str) -> &'static str {
    match name {
        "plus" => "M10 4.5v11M4.5 10h11",
        "trash" => {
            "M4.5 6.5h11M8 6.5V5a1 1 0 0 1 1-1h2a1 1 0 0 1 1 1v1.5M6 6.5l.6 8a1 1 0 0 0 1 .9h4.8a1 1 0 0 0 1-.9l.6-8"
        }
        "chevron-left" => "M12 5.5 7.5 10l4.5 4.5",
        "chevron-down" => "M5.5 8 10 12.5 14.5 8",
        "arrow-up" => "M10 15.5v-11M5.5 9 10 4.5 14.5 9",
        "stop" => "M7 7h6v6H7z",
        "copy" => {
            "M7.5 7.5V5.6a1.1 1.1 0 0 1 1.1-1.1h5.8a1.1 1.1 0 0 1 1.1 1.1v5.8a1.1 1.1 0 0 1-1.1 1.1H12.5M5.6 7.5h5.8a1.1 1.1 0 0 1 1.1 1.1v5.8a1.1 1.1 0 0 1-1.1 1.1H5.6a1.1 1.1 0 0 1-1.1-1.1V8.6a1.1 1.1 0 0 1 1.1-1.1Z"
        }
        "refresh" => "M15.5 8.5a5.5 5.5 0 1 0-1.3 5.2M15.5 4.5v4h-4",
        "check" => "M5 10.5l3.2 3.2L15 6.8",
        "eye" => {
            "M2.8 10s2.9-4.6 7.2-4.6S17.2 10 17.2 10s-2.9 4.6-7.2 4.6S2.8 10 2.8 10Zm7.2 2a2 2 0 1 0 0-4 2 2 0 0 0 0 4Z"
        }
        "eye-off" => {
            "M4 4l12 12M8.2 8.3A2 2 0 0 0 10 12a2 2 0 0 0 1.8-1.1M6.1 6.3C4 7.7 2.8 10 2.8 10s2.9 4.6 7.2 4.6c1.2 0 2.3-.35 3.2-.9M11.6 5.7C11.1 5.5 10.6 5.4 10 5.4c-.5 0-1 .06-1.4.17M17.2 10s-.85-1.35-2.3-2.6"
        }
        "bell" => {
            "M10 3.6a4.4 4.4 0 0 0-4.4 4.4c0 3-.9 4-1.5 4.8a.5.5 0 0 0 .4.85h11a.5.5 0 0 0 .4-.85c-.6-.8-1.5-1.8-1.5-4.8A4.4 4.4 0 0 0 10 3.6ZM8.5 16.1a1.7 1.7 0 0 0 3 0"
        }
        "download" => "M10 4v8M6.5 8.8 10 12.3l3.5-3.5M4.5 15.5h11",
        // 深度报告：一页带折角的文稿，内含三行正文。
        "file" => {
            "M11.5 3.5H6.4a.9.9 0 0 0-.9.9v11.2a.9.9 0 0 0 .9.9h7.2a.9.9 0 0 0 .9-.9V6.5Zm0 0V6.5h3M8 10h4M8 12.8h4"
        }
        "search" => "M9 14.5a5.5 5.5 0 1 0 0-11 5.5 5.5 0 0 0 0 11ZM13.2 13.2l3 3",
        _ => "M10 5v10M5 10h10",
    }
}

/// 是否用填充而不是描边渲染（实心块状图标）。
fn is_filled(name: &str) -> bool {
    matches!(name, "stop")
}

/// 一枚线性图标。`name` 取 [`path_for`] 支持的键；装饰性图标由外层控件提供无障碍名称。
#[component]
pub fn Icon(name: &'static str) -> impl IntoView {
    let filled = is_filled(name);
    view! {
        <svg
            viewBox="0 0 20 20"
            fill=if filled { "currentColor" } else { "none" }
            stroke=if filled { "none" } else { "currentColor" }
            stroke-width="1.5"
            stroke-linecap="round"
            stroke-linejoin="round"
            xmlns="http://www.w3.org/2000/svg"
            aria-hidden="true"
        >
            <path d=path_for(name) />
        </svg>
    }
}

/// Echo 主标识：环形回声。所有产品入口共用同一枚。
#[component]
pub fn EchoMark() -> impl IntoView {
    view! {
        <svg
            class="echo-mark-svg"
            viewBox="0 0 52 52"
            fill="none"
            xmlns="http://www.w3.org/2000/svg"
            aria-hidden="true"
        >
            <defs>
                <linearGradient id="echo-ring" x1="8" y1="5" x2="44" y2="48" gradientUnits="userSpaceOnUse">
                    <stop stop-color="#003d80"/>
                    <stop offset=".5" stop-color="#0071e3"/>
                    <stop offset="1" stop-color="#7ab6ff"/>
                </linearGradient>
            </defs>
            <path
                d="M39.3 12.2A20.3 20.3 0 1 0 40 39.1"
                stroke="#dfeaf7"
                stroke-width="6.5"
                stroke-linecap="round"
            />
            <path
                d="M39.3 12.2A20.3 20.3 0 1 0 40 39.1"
                stroke="url(#echo-ring)"
                stroke-width="3.6"
                stroke-linecap="round"
            />
            <path
                d="M17.2 27.5c3.1-6.1 8.8-9.5 14.2-8.2 2.8.7 4.7 2.4 6.4 4.2"
                stroke="#0071e3"
                stroke-width="3.4"
                stroke-linecap="round"
            />
            <path
                d="M23.4 33.4c3.5 1.9 7.9 1.5 11.2-1.5"
                stroke="#4a9bff"
                stroke-width="3.4"
                stroke-linecap="round"
            />
            <circle cx="16.4" cy="27.7" r="2.1" fill="#4a9bff"/>
        </svg>
    }
}

/// 抽象回声图形：同心细弧，呼吸极缓。登录页与研究空态共用同一枚品牌图形，
/// 让两个首屏说同一种视觉语言。
#[component]
pub fn EchoArt(#[prop(into)] class: String) -> impl IntoView {
    view! {
        <svg class=class viewBox="0 0 400 400" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
            <circle class="echo-ripple" cx="200" cy="200" r="58"/>
            <circle class="echo-ripple" cx="200" cy="200" r="96"/>
            <circle class="echo-ripple" cx="200" cy="200" r="134"/>
            <circle class="echo-ripple" cx="200" cy="200" r="172"/>
            <circle cx="200" cy="200" r="14" fill="currentColor" stroke="none" opacity=".5"/>
        </svg>
    }
}
