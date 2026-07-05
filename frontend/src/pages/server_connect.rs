//! Pre-login "Connect to server" screen (mobile).
//!
//! Lets the user enter the backend base URL, probes it for reachability,
//! persists it, then advances to the login form. On web the base is
//! same-origin, so this route is meaningless and the page just redirects to
//! the landing route.

use dioxus::prelude::*;
use dioxus_router::use_navigator;

use crate::Route;
#[cfg(feature = "mobile")]
use crate::{
    components::auth::{Banner, BannerKind, Field},
    use_server_url, use_server_url_signal,
};

/// Renders the connect-to-server page. Body branches per target (mobile owns
/// the real UI; web/SSR redirects to `/`) — the same target-split pattern as
/// [`super::auth::LoginPage`].
#[component]
pub fn ServerConnectPage() -> Element {
    #[cfg(not(feature = "mobile"))]
    {
        // Same-origin on web/SSR — nothing to configure. SSR renders the
        // empty placeholder; the client bounces to the landing route after
        // hydration (identical markup either way, so hydration is clean).
        let nav = use_navigator();
        use_effect(move || {
            nav.replace(Route::Landing {});
        });
        rsx! { div { class: "screen" } }
    }

    #[cfg(feature = "mobile")]
    {
        server_connect_mobile()
    }
}

/// The mobile Connect screen: brand shell, a server-URL field, and a Connect
/// button that validates reachability before advancing to `/login`.
#[cfg(feature = "mobile")]
fn server_connect_mobile() -> Element {
    let nav = use_navigator();
    // Prefill with the current URL (empty on first run) so the Back button
    // from Login returns here with the value already in the field.
    let mut url_input = use_signal(use_server_url);
    let mut error = use_signal(|| Option::<String>::None);
    let mut checking = use_signal(|| false);
    let url_signal = use_server_url_signal();

    let connect = use_callback(move |_: ()| {
        if checking() {
            return;
        }
        let Some(base) = normalize_server_url(&url_input()) else {
            error.set(Some("Enter a server URL.".into()));
            return;
        };
        error.set(None);
        checking.set(true);
        let mut url_signal = url_signal;
        spawn(async move {
            let res = crate::data::check_server(&base).await;
            checking.set(false);
            match res {
                Ok(()) => {
                    // Persist to disk and update the reactive context so every
                    // downstream reader picks up the new origin, then advance.
                    crate::data::server_url_store::set(&base);
                    url_signal.set(base);
                    nav.replace(Route::Login {});
                }
                Err(_) => error.set(Some(
                    "Can't reach that server. Check the address and that it's running.".into(),
                )),
            }
        });
    });

    super::auth::m_auth_shell(
        "Connect to your server to get started.",
        rsx! {
            form { class: "auth-form-inner",
                onsubmit: move |e: FormEvent| {
                    e.prevent_default();
                    connect.call(());
                },
                "data-testid": "server-connect-form",
                if let Some(msg) = error() {
                    Banner { kind: BannerKind::Err, title: msg, dismissible: false }
                }
                Field {
                    label: "Server URL".to_string(),
                    input_id: "server-url".to_string(),
                    hint: "The address where your Omnibus server is running — a local network address or a domain you host it on.".to_string(),
                    input {
                        id: "server-url",
                        name: "server-url",
                        r#type: "url",
                        inputmode: "url",
                        placeholder: "https://omnibus.local:3000",
                        autocapitalize: "none",
                        autocorrect: "off",
                        spellcheck: "false",
                        value: "{url_input}",
                        oninput: move |e| url_input.set(e.value()),
                        onkeydown: move |e: Event<KeyboardData>| {
                            if e.key() == Key::Enter {
                                e.prevent_default();
                                connect.call(());
                            }
                        },
                    }
                }
                button {
                    class: "btn primary lg auth-submit",
                    r#type: "submit",
                    disabled: checking(),
                    if checking() { "Connecting…" } else { "Connect" }
                }
                p { class: "auth-footer",
                    "Don't have a server? "
                    // TODO: point at dedicated hosting docs once they exist.
                    a {
                        class: "auth-field-action-link",
                        href: "https://github.com/seamus-sloan/omnibus",
                        target: "_blank",
                        rel: "noreferrer",
                        "Learn how to host one"
                    }
                }
            }
        },
    )
}

/// Normalize raw user input into a base URL suitable for `{base}/api/...`:
/// trims surrounding whitespace, prepends `https://` when no scheme is
/// present, and strips trailing slashes. Returns `None` for empty input.
#[cfg(feature = "mobile")]
fn normalize_server_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Detect a scheme by the `://` delimiter, not an `http` substring, so a
    // host literally named e.g. `httpserver.local` still gets a scheme.
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    Some(with_scheme.trim_end_matches('/').to_string())
}

/// Reduce a normalized base URL to a `host:port` for the login screen's
/// connected-to bar — strips the leading `scheme://` if present.
#[cfg(feature = "mobile")]
pub(crate) fn display_host(base: &str) -> String {
    base.split_once("://")
        .map_or(base, |(_, rest)| rest)
        .trim_end_matches('/')
        .to_string()
}

#[cfg(all(test, feature = "mobile"))]
mod tests {
    use super::*;

    #[test]
    fn normalize_prepends_https_when_no_scheme() {
        assert_eq!(
            normalize_server_url("omnibus.local:3000").as_deref(),
            Some("https://omnibus.local:3000")
        );
    }

    #[test]
    fn normalize_preserves_explicit_scheme() {
        assert_eq!(
            normalize_server_url("http://127.0.0.1:3000").as_deref(),
            Some("http://127.0.0.1:3000")
        );
        assert_eq!(
            normalize_server_url("https://books.example").as_deref(),
            Some("https://books.example")
        );
    }

    #[test]
    fn normalize_trims_whitespace_and_trailing_slashes() {
        assert_eq!(
            normalize_server_url("  https://books.example:9/// ").as_deref(),
            Some("https://books.example:9")
        );
    }

    #[test]
    fn normalize_rejects_empty_or_whitespace() {
        assert_eq!(normalize_server_url(""), None);
        assert_eq!(normalize_server_url("   \t\n"), None);
    }

    #[test]
    fn normalize_scheme_detection_uses_delimiter_not_substring() {
        assert_eq!(
            normalize_server_url("httpserver.local").as_deref(),
            Some("https://httpserver.local")
        );
    }

    #[test]
    fn display_host_strips_scheme_keeps_host_and_port() {
        assert_eq!(
            display_host("https://omnibus.local:3000"),
            "omnibus.local:3000"
        );
        assert_eq!(display_host("http://127.0.0.1:3000"), "127.0.0.1:3000");
    }

    #[test]
    fn display_host_passes_bare_host_through() {
        assert_eq!(display_host("omnibus.local:3000"), "omnibus.local:3000");
    }
}
