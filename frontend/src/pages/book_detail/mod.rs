//! Book detail page — Atrium "Cinematic" redesign.
//!
//! Composes Atrium primitives ([`crate::components::atrium::Cover`], buttons,
//! cards, chips, dividers) into the layout sketched in
//! `screens/book-detail.jsx#DetailA` from the Omnibus design canvas. Sub-
//! modules carry the section bodies — [`hero`] (cover + title + CTAs +
//! rating card), [`body`] (two-column main + sticky rail), and [`merge`]
//! (admin "Merge with…" dialog and post-merge toast on web). This shell
//! owns the data fetch, the breadcrumb/title plumbing, and the small
//! markup primitives reused by every section.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::EbookMetadata;
#[cfg(not(feature = "mobile"))]
use omnibus_shared::MergeBooksResult;

use crate::{data, use_server_url, Route};

mod body;
mod hero;
mod merge;

use body::{BdBodyMain, BdRailSection};
use hero::BdHeroSection;

/// Book detail page shell: fetches metadata then hands off to `render_loaded`.
#[component]
pub fn BookDetailPage(uuid: String) -> Element {
    let server_url = use_server_url();
    let mut book: Signal<Option<EbookMetadata>> = use_signal(|| None);
    let mut author_books: Signal<Vec<EbookMetadata>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    // Bumped after a merge/undo so the effect below refetches the book
    // (signals read inside `use_effect` re-arm it).
    let refresh = use_signal(|| 0u32);

    // Admin gating for the "Merge with…" affordance. The signal and the
    // effect are declared unconditionally so SSR and the hydrating WASM
    // bundle agree on hook count and order — Dioxus tracks hooks
    // positionally, and a cfg-gated declaration would diverge the two.
    // The effect itself only runs on the client (Dioxus skips `use_effect`
    // during SSR), and `data::current_user()` resolves to an
    // `Ok(None)`-returning stub under server-only builds, so no admin
    // surface ever leaks into the prerendered markup. Mobile renders no
    // admin surfaces; the server-side `AdminUser` extractor on
    // `rpc_merge_books` is the actual security boundary.
    let mut is_admin = use_signal(|| false);
    use_effect(move || {
        spawn(async move {
            if let Ok(Some(user)) = data::current_user().await {
                is_admin.set(user.is_admin);
            }
        });
    });

    #[cfg(not(feature = "mobile"))]
    let merge_open = use_signal(|| false);
    #[cfg(not(feature = "mobile"))]
    let merge_result: Signal<Option<MergeBooksResult>> = use_signal(|| None);
    #[cfg(not(feature = "mobile"))]
    let undo_error: Signal<Option<String>> = use_signal(|| None);

    let url = server_url.clone();
    use_effect(use_reactive!(|uuid| {
        let _ = refresh();
        let url = url.clone();
        let uuid = uuid.clone();
        spawn(async move {
            loading.set(true);
            author_books.set(Vec::new());
            match data::get_ebook(&url, &uuid).await {
                Ok(b) => {
                    let author_fetch = b.as_ref().map(|inner| {
                        (
                            inner.creators.first().and_then(|c| c.id),
                            inner.unique_identifier.clone(),
                        )
                    });
                    book.set(b);
                    error.set(None);
                    loading.set(false);
                    if let Some((Some(aid), current_uuid)) = author_fetch {
                        if let Ok(Some(ad)) = data::get_author(&url, aid).await {
                            let still_current =
                                book().as_ref().and_then(|b| b.unique_identifier.as_ref())
                                    == current_uuid.as_ref();
                            if still_current {
                                let others: Vec<EbookMetadata> = ad
                                    .books
                                    .into_iter()
                                    .filter(|ab| ab.unique_identifier != current_uuid)
                                    .collect();
                                author_books.set(others);
                            }
                        }
                    }
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                    loading.set(false);
                }
            }
        });
    }));

    if loading() {
        return rsx! {
            p { class: "subtitle", "Loading\u{2026}" }
        };
    }
    if let Some(msg) = error() {
        return rsx! {
            p { role: "alert", class: "subtitle", "{msg}" }
            Link { to: Route::Landing {}, class: "btn", "Back to library" }
        };
    }
    let Some(b) = book() else {
        return rsx! {
            p { class: "subtitle", "Book not found." }
            Link { to: Route::Landing {}, class: "btn", "Back to library" }
        };
    };

    // `is_admin` starts at `false` and only flips to `true` on the web
    // client after `current_user()` resolves an admin user, so this read
    // returns `false` during SSR and for non-admins on every platform.
    let is_admin_flag = is_admin();
    #[cfg_attr(feature = "mobile", allow(unused_variables))]
    let page_title = b.title.clone().unwrap_or_else(|| b.filename.clone());
    #[cfg_attr(feature = "mobile", allow(unused_variables))]
    let page_uuid = b.unique_identifier.clone().unwrap_or_default();

    // Rail "Merge with…" button (admin, web only) — threaded down as a
    // prebuilt Element so the rail component stays platform-agnostic.
    #[cfg(not(feature = "mobile"))]
    let merge_button: Option<Element> = merge::build_merge_button(is_admin_flag, merge_open);
    #[cfg(feature = "mobile")]
    let merge_button: Option<Element> = merge::build_merge_button(is_admin_flag);

    #[cfg(not(feature = "mobile"))]
    let merge_ui: Option<Element> = merge::build_merge_ui(
        merge_open,
        merge_result,
        undo_error,
        refresh,
        server_url.clone(),
        page_uuid,
        page_title,
    );
    #[cfg(feature = "mobile")]
    let merge_ui: Option<Element> = merge::build_merge_ui();

    let body = render_loaded(b, author_books(), merge_button);
    rsx! {
        {body}
        {merge_ui}
    }
}

// ---------------------------------------------------------------------------
// View — split out so the loaded-book case is the only thing rendered and
// the data-fetch shell stays small.
// ---------------------------------------------------------------------------

fn kicker_label(year: &str) -> String {
    if year.is_empty() {
        "Book".to_string()
    } else {
        format!("Book · {year}")
    }
}

fn series_label(series: Option<&str>, index: Option<&str>) -> Option<String> {
    match (series, index) {
        (Some(s), Some(i)) => Some(format!("{s} #{i}")),
        (Some(s), None) => Some(s.to_string()),
        _ => None,
    }
}

/// Build the breadcrumb item list for a loaded book.
fn build_crumbs(
    b: &EbookMetadata,
    title: &str,
    primary_author: &str,
    series_label: &Option<String>,
) -> Vec<BdCrumbItem> {
    let mut crumbs = vec![BdCrumbItem {
        text: "Home".to_string(),
        target: Some(Route::Landing {}),
    }];
    if !primary_author.is_empty() {
        let author_route = b
            .creators
            .first()
            .and_then(|c| c.id)
            .map(|id| Route::AuthorDetail { id });
        crumbs.push(BdCrumbItem {
            text: primary_author.to_string(),
            target: author_route,
        });
    }
    if let Some(label) = series_label.clone() {
        let series_route = b.series_id.map(|id| Route::SeriesDetail { id });
        crumbs.push(BdCrumbItem {
            text: label,
            target: series_route,
        });
    }
    crumbs.push(BdCrumbItem {
        text: title.to_string(),
        target: None,
    });
    crumbs
}

/// Render the fully-loaded book detail view.
fn render_loaded(
    b: EbookMetadata,
    author_books: Vec<EbookMetadata>,
    merge_button: Option<Element>,
) -> Element {
    let title = b.title.clone().unwrap_or_else(|| b.filename.clone());
    let primary_author = b
        .creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let authors_line = b
        .creators
        .iter()
        .map(|c| c.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let year = b
        .published
        .as_deref()
        .and_then(|p| p.get(0..4))
        .unwrap_or("")
        .to_string();
    let kicker = kicker_label(&year);
    let series = series_label(b.series.as_deref(), b.series_index.as_deref());
    let accent_style = b
        .accent
        .as_deref()
        .map(|a| format!("--accent: {a};"))
        .unwrap_or_default();

    let has_audio = b
        .formats
        .iter()
        .any(|f| f.eq_ignore_ascii_case("m4b") || f.eq_ignore_ascii_case("mp3"));
    let has_ebook = b
        .formats
        .iter()
        .any(|f| f.eq_ignore_ascii_case("epub") || f.eq_ignore_ascii_case("pdf"));

    let crumbs = build_crumbs(&b, &title, &primary_author, &series);

    rsx! {
        div { class: "bd-root", style: "{accent_style}",
            BdHeroSection {
                b: b.clone(),
                title: title.clone(),
                kicker: kicker.clone(),
                crumbs,
                has_ebook,
                has_audio,
            }
            section { class: "bd-body-grid",
                BdBodyMain {
                    title: title.clone(),
                    primary_author: primary_author.clone(),
                    author_books: author_books.clone(),
                }
                BdRailSection {
                    b: b.clone(),
                    title: title.clone(),
                    authors_line: authors_line.clone(),
                    series: series.clone(),
                    merge_button,
                }
            }
            div { class: "bd-footer",
                Link { to: Route::Landing {}, class: "btn", "Back to library" }
            }
            // Hidden slots preserved for the F1.4 contract — the hero
            // rating card and the cover-fan strips are the visible
            // surfaces; these stay attached so anything keying off the
            // slot testids still finds them.
            div {
                class: "ratings-slot",
                "data-testid": "ratings-slot",
                aria_label: "Ratings \u{2014} coming soon",
                hidden: true,
            }
            div {
                class: "suggestions-slot",
                "data-testid": "suggestions-slot",
                aria_label: "Suggestions \u{2014} coming soon",
                hidden: true,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Page-local primitives. None of these introduce business logic — they're
// markup-only adapters so the page reads as a composition of named blocks
// rather than nested rsx.
// ---------------------------------------------------------------------------

/// One breadcrumb segment. When `target` is `Some`, the segment renders as a
/// router `Link`; otherwise it's a plain `<span>` for the current page or a
/// segment without a resolvable detail route.
#[derive(Clone, PartialEq, Props)]
pub struct BdCrumbItem {
    pub text: String,
    pub target: Option<Route>,
}

/// Atrium-styled breadcrumb. Segments with a `target` route render as Links,
/// otherwise as plain spans. The final segment is always rendered as the
/// "current page" span regardless of target.
#[component]
pub(super) fn BdCrumb(items: Vec<BdCrumbItem>) -> Element {
    let last_idx = items.len().saturating_sub(1);
    rsx! {
        nav { class: "bd-crumb", "aria-label": "breadcrumb",
            for (i, item) in items.iter().cloned().enumerate() {
                if i > 0 {
                    span { class: "bd-crumb-sep", "\u{203a}" }
                }
                if i == last_idx {
                    span { class: "bd-crumb-curr", "{item.text}" }
                } else if let Some(route) = item.target.clone() {
                    Link {
                        to: route,
                        class: if i == 0 { "bd-crumb-home" } else { "bd-crumb-step" },
                        "{item.text}"
                    }
                } else {
                    span { class: "bd-crumb-step", "{item.text}" }
                }
            }
        }
    }
}

/// Body section heading row — kicker label + serif title. The kicker stacks
/// above the title (mirrors `screens/_shared.jsx#SectionHead`).
#[component]
pub(super) fn BdSectionHead(kicker: String, title: String) -> Element {
    rsx! {
        div { class: "bd-section-head",
            div { class: "bd-section-head-text",
                if !kicker.is_empty() {
                    div { class: "label bd-section-kicker", "{kicker}" }
                }
                h3 { class: "bd-section-title", "{title}" }
            }
        }
    }
}

/// Small monospace format pill (matches `screens/_shared.jsx#FormatBadge`).
#[component]
pub(super) fn BdFormatBadge(fmt: String) -> Element {
    rsx! {
        div { class: "bd-fmt-badge", "data-testid": "bd-format-badge", "{fmt}" }
    }
}

/// Read-only star display. Half-filled stars are rounded down to nearest
/// integer in the stub; the interactive widget will replace this later.
#[component]
pub(super) fn BdStars(value: f32) -> Element {
    let full = value.floor().clamp(0.0, 5.0) as u32;
    rsx! {
        span { class: "bd-stars-row",
            for i in 0..5u32 {
                span {
                    class: if i < full { "bd-star bd-star-on" } else { "bd-star" },
                    "\u{2605}"
                }
            }
        }
    }
}

#[component]
pub(super) fn BdMetaRow(k: String, v: String) -> Element {
    rsx! {
        tr { class: "bd-meta-row",
            td { class: "bd-meta-k", "{k}" }
            td { class: "bd-meta-v", "{v}" }
        }
    }
}

#[component]
pub(super) fn BdInsightCell(label: String, value: String) -> Element {
    rsx! {
        div { class: "bd-insight-cell",
            div { class: "mono bd-insight-label", "{label}" }
            div { class: "bd-insight-value", "{value}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kicker_label_renders_year_when_present() {
        assert_eq!(kicker_label("2021"), "Book · 2021");
    }
    #[test]
    fn kicker_label_falls_back_to_book_when_year_empty() {
        assert_eq!(kicker_label(""), "Book");
    }
    #[test]
    fn series_label_formats_name_and_index() {
        assert_eq!(
            series_label(Some("Dune"), Some("2")),
            Some("Dune #2".into())
        );
    }
    #[test]
    fn series_label_without_index_is_just_name() {
        assert_eq!(series_label(Some("Dune"), None), Some("Dune".into()));
    }
    #[test]
    fn series_label_absent_series_is_none() {
        assert_eq!(series_label(None, Some("2")), None);
        assert_eq!(series_label(None, None), None);
    }
}
