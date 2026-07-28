//! 模型答案的 Markdown 渲染——纯 Markdown 语义标签白名单，禁止任何原始 HTML 透传。
//!
//! `pulldown-cmark` 默认会把源文本里的原始 HTML（HTML block / inline HTML）原样透传进输出，
//! 这对模型生成文本是不可接受的注入面。这里把解析事件流过滤成白名单：只保留 Markdown 语义
//! 事件（段落/标题/强调/列表/代码/链接等），`Html`/`InlineHtml` 事件整块丢弃；链接与图片的
//! `dest_url` 额外校验协议，拒绝 `javascript:`/`data:` 等可执行 scheme。

use pulldown_cmark::{Event, Options, Parser, Tag, html};

/// 把模型答案文本渲染成消毒后的 HTML 字符串，供 `inner_html` 直接注入。
pub fn render(text: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);

    let sanitized = Parser::new_ext(text, options).filter_map(sanitize_event);
    let mut out = String::new();
    html::push_html(&mut out, sanitized);
    out
}

fn sanitize_event(event: Event<'_>) -> Option<Event<'_>> {
    match event {
        // 禁 raw HTML：整块丢弃，而不是退化成纯文本——避免片段拼接后仍能凑出可执行标签。
        Event::Html(_) | Event::InlineHtml(_) => None,
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Some(Event::Start(Tag::Link {
            link_type,
            dest_url: sanitize_url(&dest_url).into(),
            title,
            id,
        })),
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => Some(Event::Start(Tag::Image {
            link_type,
            dest_url: sanitize_url(&dest_url).into(),
            title,
            id,
        })),
        other => Some(other),
    }
}

/// 只放行 http(s)/mailto/站内相对路径；其余（含 `javascript:`/`data:`）一律归一到 `#`。
fn sanitize_url(url: &str) -> String {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || trimmed.starts_with('#')
        || trimmed.starts_with('/')
    {
        trimmed.to_string()
    } else {
        "#".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_basic_markdown_semantics() {
        let html = render("**粗体** 与 *斜体*，以及一个列表：\n\n- 一\n- 二");
        assert!(html.contains("<strong>粗体</strong>"));
        assert!(html.contains("<em>斜体</em>"));
        assert!(html.contains("<li>一</li>"));
    }

    #[test]
    fn strips_html_block_entirely() {
        let html = render("正文\n\n<script>alert(1)</script>\n\n结尾");
        assert!(!html.contains("<script>"));
        assert!(!html.contains("alert(1)"));
        assert!(html.contains("正文"));
        assert!(html.contains("结尾"));
    }

    #[test]
    fn strips_inline_html_tag_but_keeps_following_text_escaped() {
        let html = render("before <img src=x onerror=alert(1)> after");
        assert!(!html.contains("<img"));
        assert!(!html.contains("onerror"));
        assert!(html.contains("before"));
        assert!(html.contains("after"));
    }

    #[test]
    fn rejects_javascript_scheme_link() {
        let html = render("[click](javascript:alert(1))");
        assert!(!html.contains("javascript:"));
        assert!(html.contains("href=\"#\""));
    }

    #[test]
    fn keeps_https_link() {
        let html = render("[echo](https://example.com/report)");
        assert!(html.contains("href=\"https://example.com/report\""));
    }
}
