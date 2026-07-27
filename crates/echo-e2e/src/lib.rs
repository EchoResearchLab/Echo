//! 真实浏览器验收。先启动 `echo-api` 与 `trunk serve`，再启动 chromedriver/geckodriver，
//! 用 `cargo test -p echo-e2e -- --ignored` 执行；没有驱动时默认不阻塞普通 Rust CI。
//!
//! 定位器一律用稳定钩子（`aria-label` / 组件类名 / 导航文案），不用 placeholder 文案——
//! 文案是文案，改一版文案不该让验收失效。此前这里挂着 `placeholder*='想研究什么'`
//! 和 `placeholder='平均成本'` 这类早已不存在的串，测试即使跑起来也是必挂的。

#[cfg(test)]
mod tests {
    use fantoccini::{Client, ClientBuilder, Locator};

    /// 按导航/切面文案点一个按钮。
    async fn click_by_text(client: &Client, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        client
            .find(Locator::XPath(&format!(
                "//button[normalize-space()='{text}']"
            )))
            .await?
            .click()
            .await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "需要运行中的 WebDriver(127.0.0.1:4444)、echo-api 与 trunk serve"]
    async fn research_library_settings_core_flow() -> Result<(), Box<dyn std::error::Error>> {
        let client = ClientBuilder::rustls()?
            .connect("http://127.0.0.1:4444")
            .await?;
        client.goto("http://127.0.0.1:5191/").await?;

        // ── 研究：提问 → 作答 ──
        // 编辑器里只有一个输入框：主体由服务端从问题文本识别，界面不再要求用户先选公司。
        client
            .find(Locator::Css("textarea[aria-label='研究问题']"))
            .await?
            .send_keys("AAPL 的估值判断")
            .await?;
        client
            .find(Locator::Css("button.composer-send"))
            .await?
            .click()
            .await?;
        // 生成中：发送按钮就地变成停止，用户不必翻回上面的卡片找刹车。
        client
            .find(Locator::Css("button.composer-send.is-stop"))
            .await?;
        // 思考态：阶段提示与从左到右的流动指示。
        client
            .find(Locator::Css(".stage-label .thinking-wave"))
            .await?;
        client.find(Locator::Css(".answer-card")).await?;
        // 答案上必须有复制、导出与重新生成入口。
        client.find(Locator::Css(".answer-actions")).await?;

        // ── 资料库：自选与监控 → 持仓 → 研究档案 ──
        click_by_text(&client, "资料库").await?;
        client.find(Locator::Css(".watch-form-grid input")).await?;
        click_by_text(&client, "持仓").await?;
        client
            .find(Locator::Css(".portfolio-form-grid input"))
            .await?;
        click_by_text(&client, "研究档案").await?;
        client.find(Locator::Css(".profiles-layout")).await?;

        // ── 设置 ──
        click_by_text(&client, "设置").await?;
        client.find(Locator::Css(".settings-card")).await?;

        client.close().await?;
        Ok(())
    }

    /// 破坏性操作走应用内确认对话框，不再是浏览器原生 confirm——原生弹窗 WebDriver
    /// 只能靠 alert API 处理，也无法验证文案是否说清了后果。
    #[tokio::test]
    #[ignore = "需要运行中的 WebDriver(127.0.0.1:4444)、echo-api 与 trunk serve"]
    async fn destructive_action_uses_in_app_confirm_dialog()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = ClientBuilder::rustls()?
            .connect("http://127.0.0.1:4444")
            .await?;
        client.goto("http://127.0.0.1:5191/").await?;

        // 悬停后出现的删除按钮——直接点即可（WebDriver 的 click 会先滚动到元素）。
        client
            .find(Locator::Css(".session-item-delete"))
            .await?
            .click()
            .await?;
        let dialog = client.find(Locator::Css(".dialog-shell")).await?;
        assert!(
            dialog.text().await?.contains("无法恢复"),
            "确认对话框必须说清后果"
        );
        // 取消后什么都不该发生。
        click_by_text(&client, "取消").await?;
        assert!(
            client.find(Locator::Css(".dialog-shell")).await.is_err(),
            "取消后对话框应关闭"
        );

        client.close().await?;
        Ok(())
    }
}
