#![cfg(feature = "e2e")]

use playwright::Playwright;

fn home_url(base: &str) -> String {
    format!("{base}/")
}

fn settings_url(base: &str) -> String {
    format!("{base}/settings")
}

#[tokio::test]
#[ignore = "Requires Playwright browser install and a running local server"]
async fn landing_page_renders() -> Result<(), Box<dyn std::error::Error>> {
    let client = Playwright::initialize().await?;
    let chromium = client.chromium();
    let browser = chromium.launcher().headless(true).launch().await?;
    let page = browser.new_page().await?;

    page.goto_builder(&home_url("http://127.0.0.1:3000"))
        .goto()
        .await?;
    let title = page.text_content("h1").await?.unwrap_or_default();
    assert!(title.contains("Counter"));

    browser.close().await?;
    client.prepare()?.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "Requires Playwright browser install and a running local server"]
async fn settings_navigation_works() -> Result<(), Box<dyn std::error::Error>> {
    let client = Playwright::initialize().await?;
    let chromium = client.chromium();
    let browser = chromium.launcher().headless(true).launch().await?;
    let page = browser.new_page().await?;

    page.goto_builder(&settings_url("http://127.0.0.1:3000"))
        .goto()
        .await?;
    let heading = page.text_content("h1").await?.unwrap_or_default();
    assert_eq!(heading, "Settings");

    browser.close().await?;
    client.prepare()?.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "Requires Playwright browser install and a running local server"]
async fn increment_updates_displayed_value() -> Result<(), Box<dyn std::error::Error>> {
    let client = Playwright::initialize().await?;
    let chromium = client.chromium();
    let browser = chromium.launcher().headless(true).launch().await?;
    let page = browser.new_page().await?;

    page.goto_builder(&home_url("http://127.0.0.1:3000"))
        .goto()
        .await?;
    let before = page
        .text_content("#current-value")
        .await?
        .unwrap_or_default();
    page.click("#increment-button").await?;
    page.wait_for_timeout(250).await?;
    let after = page
        .text_content("#current-value")
        .await?
        .unwrap_or_default();

    let before_val = before.parse::<i64>().unwrap_or_default();
    let after_val = after.parse::<i64>().unwrap_or_default();
    assert!(after_val >= before_val + 1);

    browser.close().await?;
    client.prepare()?.close().await?;
    Ok(())
}
