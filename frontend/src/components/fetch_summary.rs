//! "Fetch Summary" button plus inline status. Fetches the server's ordered
//! source plan, drives it from the client so it can show a per-source
//! "Searching…" message, and hands the resolved text to `on_fetched` — the
//! caller decides whether to fill an editor field or save and refresh. SSR and
//! first WASM paint render the same idle markup; network work runs on click.

use dioxus::prelude::*;
use omnibus_shared::summary::SummarySource;

use crate::{data, use_server_url};

/// The sparkle glyph on the Fetch Summary button, drawn in the accent colour
/// (mirrors `book_detail::immersive`'s mark treatment). An SVG icon, not an
/// emoji, so it inherits `currentColor` and stays crisp at any size.
fn fetch_summary_mark() -> Element {
    rsx! {
        span { class: "fs-fetch-mark", aria_hidden: "true",
            svg {
                width: "16",
                height: "16",
                view_box: "0 0 24 24",
                fill: "none",
                stroke: "currentColor",
                "stroke-width": "1.7",
                "stroke-linecap": "round",
                "stroke-linejoin": "round",
                path { d: "M12 3l1.7 4.8L18.5 9.5l-4.8 1.7L12 16l-1.7-4.8L5.5 9.5l4.8-1.7z" }
                path { d: "M18.5 14.5l.85 2.15L21.5 17.5l-2.15.85L18.5 20.5l-.85-2.15L15.5 17.5l2.15-.85z" }
            }
        }
    }
}

/// What the button is currently doing — mirrored into the inline status line.
#[derive(Clone, PartialEq)]
enum FetchState {
    Idle,
    Searching(SummarySource),
    Error(String),
}

/// A "Fetch Summary" button that pulls a book blurb from the configured source
/// cascade (Hardcover → Google Books → Open Library), showing a per-source
/// spinner + status. On success it calls `on_fetched` with the text.
#[component]
pub fn FetchSummaryButton(uuid: String, on_fetched: EventHandler<String>) -> Element {
    let server_url = use_server_url();
    let state = use_signal(|| FetchState::Idle);
    // Set synchronously at click, before the first `await`, so a rapid second
    // click can't spawn a concurrent cascade (the `state` flag only flips to
    // `Searching` after the source plan resolves, which is too late).
    let mut busy = use_signal(|| false);

    let on_click = move |_| {
        if busy() {
            return;
        }
        busy.set(true);
        let url = server_url.clone();
        let uuid = uuid.clone();
        spawn(async move {
            if let Some(text) = run_cascade(&url, &uuid, state).await {
                on_fetched.call(text);
                let mut state = state;
                state.set(FetchState::Idle);
            }
            busy.set(false);
        });
    };

    let running = busy();

    rsx! {
        div { class: "fs-fetch",
            button {
                r#type: "button",
                class: "btn fs-fetch-btn",
                "data-testid": "fetch-summary",
                disabled: running,
                onclick: on_click,
                {fetch_summary_mark()}
                "Fetch Summary"
            }
            match state() {
                FetchState::Searching(source) => rsx! {
                    div {
                        class: "fs-fetch-status",
                        role: "status",
                        "data-testid": "fetch-summary-status",
                        span { class: "suggest-spinner", aria_hidden: "true" }
                        span { "{searching_label(source)}" }
                    }
                },
                FetchState::Error(msg) => rsx! {
                    div {
                        class: "fs-fetch-error",
                        role: "alert",
                        "data-testid": "fetch-summary-error",
                        "{msg}"
                    }
                },
                FetchState::Idle => rsx! {},
            }
        }
    }
}

/// Walk the server's ordered source plan, updating `state` with the per-source
/// "Searching…" message. Returns the found text, or `None` (leaving `state` on
/// an error) when no source had a summary. A miss OR a transient error on one
/// source falls through to the next — the goal is to surface *a* summary if one
/// exists anywhere; only the final source's outcome decides the error shown.
async fn run_cascade(url: &str, uuid: &str, mut state: Signal<FetchState>) -> Option<String> {
    // A failed plan fetch is a transport/RPC error, not a summary miss — surface
    // it distinctly rather than collapsing it into "No summary found."
    let sources = match data::summary_sources(url).await {
        Ok(sources) => sources,
        Err(e) => {
            state.set(FetchState::Error(format!(
                "Couldn't fetch summary sources: {e}"
            )));
            return None;
        }
    };
    // The server always includes at least Google Books, so an empty plan is
    // unexpected — treat it as a config problem, not a summary miss.
    if sources.is_empty() {
        state.set(FetchState::Error(
            "Couldn't fetch a summary: no sources are configured.".to_string(),
        ));
        return None;
    }

    // Only the final source's outcome decides the error shown; a miss or a
    // transient error on an earlier source just falls through to the next.
    let last = sources.len() - 1;
    for (i, source) in sources.into_iter().enumerate() {
        state.set(FetchState::Searching(source));
        match data::fetch_summary(url, uuid, source).await {
            Ok(Some(text)) => return Some(text),
            Ok(None) if i == last => {
                state.set(FetchState::Error("No summary found.".to_string()));
            }
            Err(e) if i == last => {
                state.set(FetchState::Error(format!("Couldn't fetch a summary: {e}")));
            }
            _ => {}
        }
    }
    None
}

/// The per-source "Searching…" status text.
fn searching_label(source: SummarySource) -> &'static str {
    match source {
        SummarySource::Hardcover => "Searching for summary on Hardcover\u{2026}",
        SummarySource::GoogleBooks => "Searching for summary on Google Books\u{2026}",
        SummarySource::OpenLibrary => "Searching for summary on OpenLibrary\u{2026}",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searching_label_names_the_source() {
        assert_eq!(
            searching_label(SummarySource::Hardcover),
            "Searching for summary on Hardcover\u{2026}"
        );
        assert_eq!(
            searching_label(SummarySource::GoogleBooks),
            "Searching for summary on Google Books\u{2026}"
        );
        assert_eq!(
            searching_label(SummarySource::OpenLibrary),
            "Searching for summary on OpenLibrary\u{2026}"
        );
    }
}
