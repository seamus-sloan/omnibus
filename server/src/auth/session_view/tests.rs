use super::*;

const FIREFOX_MAC: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:133.0) \
                           Gecko/20100101 Firefox/133.0";
const CHROME_WINDOWS: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                              (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const EDGE_WINDOWS: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                            (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0";
const SAFARI_IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) \
                             AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 \
                             Mobile/15E148 Safari/604.1";
const FIREFOX_IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) \
                              AppleWebKit/605.1.15 (KHTML, like Gecko) FxiOS/133.0 \
                              Mobile/15E148 Safari/605.1.15";
const CHROME_ANDROID: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 \
                              (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36";

#[test]
fn client_label_names_the_browser_and_os_from_a_user_agent() {
    assert_eq!(
        client_label(None, Some(FIREFOX_MAC)),
        "Firefox on macOS".to_string()
    );
    assert_eq!(
        client_label(None, Some(CHROME_WINDOWS)),
        "Chrome on Windows".to_string()
    );
}

#[test]
fn client_label_prefers_a_registered_device_name_over_the_user_agent() {
    assert_eq!(
        client_label(Some("Seamus's iPhone"), Some(FIREFOX_MAC)),
        "Seamus's iPhone".to_string()
    );
}

#[test]
fn client_label_ignores_a_blank_device_name() {
    assert_eq!(
        client_label(Some("   "), Some(FIREFOX_MAC)),
        "Firefox on macOS".to_string()
    );
}

#[test]
fn client_label_falls_back_to_unknown_without_a_device_or_user_agent() {
    assert_eq!(client_label(None, None), UNKNOWN.to_string());
    assert_eq!(client_label(None, Some("curl/8.7.1")), UNKNOWN.to_string());
}

#[test]
fn client_label_reports_the_os_alone_when_no_browser_family_matches() {
    // URLSession's default header for a native Apple client: no browser
    // token, but the platform is still recoverable.
    assert_eq!(
        client_label(
            None,
            Some("omnibus/12 CFNetwork/1568.100.1 Darwin/24.1.0 (iPhone)")
        ),
        "iOS".to_string()
    );
}

#[test]
fn browser_name_resolves_families_that_impersonate_each_other() {
    // Every one of these claims at least one family listed below it.
    assert_eq!(browser_name(EDGE_WINDOWS), Some("Edge"));
    assert_eq!(browser_name(CHROME_WINDOWS), Some("Chrome"));
    assert_eq!(browser_name(FIREFOX_IPHONE), Some("Firefox"));
    assert_eq!(browser_name(SAFARI_IPHONE), Some("Safari"));
}

#[test]
fn browser_name_returns_none_for_a_non_browser_agent() {
    assert_eq!(browser_name("omnibus-mcp/0.1.0"), None);
}

#[test]
fn os_name_prefers_the_specific_platform_over_the_kernel_it_claims() {
    // iOS says "like Mac OS X"; Android says "Linux".
    assert_eq!(os_name(SAFARI_IPHONE), Some("iOS"));
    assert_eq!(os_name(CHROME_ANDROID), Some("Android"));
    assert_eq!(os_name(FIREFOX_MAC), Some("macOS"));
}

#[test]
fn os_name_returns_none_for_an_agent_that_names_no_platform() {
    assert_eq!(os_name("curl/8.7.1"), None);
}

#[tokio::test]
async fn session_views_names_each_row_and_flags_only_the_current_one() {
    use omnibus_db::auth::SessionKind;

    let pool = omnibus_db::init_db("sqlite::memory:").await.unwrap();
    let alice = crate::auth::test_support::create_user(&pool, "alice").await;
    let phone = omnibus_db::auth::register_device(&pool, alice.id, "Alice's iPhone", "ios", None)
        .await
        .unwrap();
    let native = omnibus_db::auth::create_session(
        &pool,
        alice.id,
        Some(phone.id),
        SessionKind::Bearer,
        3600,
        Some("omnibus/12 CFNetwork/1568.100.1 Darwin/24.1.0"),
    )
    .await
    .unwrap();
    let web = omnibus_db::auth::create_session(
        &pool,
        alice.id,
        None,
        SessionKind::Cookie,
        3600,
        Some(CHROME_WINDOWS),
    )
    .await
    .unwrap();

    let rows = omnibus_db::auth::list_sessions_for_user(&pool, alice.id)
        .await
        .unwrap();
    let views = session_views(&pool, alice.id, rows, Some(web.session.id))
        .await
        .unwrap();

    let native_view = views.iter().find(|v| v.id == native.session.id).unwrap();
    assert_eq!(native_view.client, "Alice's iPhone");
    assert!(!native_view.is_current);

    let web_view = views.iter().find(|v| v.id == web.session.id).unwrap();
    assert_eq!(web_view.client, "Chrome on Windows");
    assert!(web_view.is_current);
}

#[tokio::test]
async fn session_views_flags_nothing_current_when_no_session_id_is_given() {
    use omnibus_db::auth::SessionKind;

    let pool = omnibus_db::init_db("sqlite::memory:").await.unwrap();
    let alice = crate::auth::test_support::create_user(&pool, "alice").await;
    omnibus_db::auth::create_session(&pool, alice.id, None, SessionKind::Cookie, 3600, None)
        .await
        .unwrap();

    let rows = omnibus_db::auth::list_sessions_for_user(&pool, alice.id)
        .await
        .unwrap();
    let views = session_views(&pool, alice.id, rows, None).await.unwrap();
    assert!(views.iter().all(|v| !v.is_current));
}
