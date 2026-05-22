//! Book detail page — Atrium "Cinematic" redesign.
//!
//! Composes Atrium primitives ([`crate::components::atrium::Cover`], buttons,
//! cards, chips, dividers) into the layout sketched in
//! `screens/book-detail.jsx#DetailA` from the Omnibus design canvas:
//!
//! * **Hero** — accent-tinted radial backdrop, 240-px cover with format
//!   badges, 76-px italic-serif title, author link, description, primary
//!   CTAs, in-progress chip bar, tag chips, and a right-side "your rating"
//!   action card.
//! * **Body** — two-column grid with journal entries, highlights, and two
//!   cover-fan rows on the left; a sticky rail on the right holding file
//!   details (with the relocated [`crate::components::FormatSwitcher`]),
//!   series/standalone info, and reading insights.
//!
//! Backend-supplied fields (title / authors / description / cover / formats
//! / subjects / identifiers / publisher / language / published) wire through
//! from `data::get_ebook`. Sections that don't yet have backend support
//! (rating, journal, highlights, more-by-author, suggestions, reading
//! activity, shelves) render placeholder content with `aria-hidden` and a
//! `TODO(F<n>.<m>)` line citing the future roadmap doc that will replace
//! the stub.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::EbookMetadata;

use crate::components::atrium::Cover;
use crate::components::FormatSwitcher;
use crate::{data, use_server_url, Route};

#[component]
pub fn BookDetailPage(id: i64) -> Element {
    let server_url = use_server_url();
    let mut book: Signal<Option<EbookMetadata>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let url = server_url.clone();
    use_effect(move || {
        let url = url.clone();
        spawn(async move {
            loading.set(true);
            match data::get_ebook(&url, id).await {
                Ok(b) => {
                    book.set(b);
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

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

    render_loaded(b)
}

// ---------------------------------------------------------------------------
// View — split out so the loaded-book case is the only thing rendered and
// the data-fetch shell stays small.
// ---------------------------------------------------------------------------

fn render_loaded(b: EbookMetadata) -> Element {
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
    let kicker = match (b.dc_type.as_deref(), year.is_empty()) {
        (Some(t), false) => format!("{t} · {year}"),
        (Some(t), true) => t.to_string(),
        (None, false) => format!("Book · {year}"),
        (None, true) => "Book".to_string(),
    };
    let series_label = match (b.series.as_deref(), b.series_index.as_deref()) {
        (Some(s), Some(i)) => Some(format!("{s} #{i}")),
        (Some(s), None) => Some(s.to_string()),
        _ => None,
    };
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

    rsx! {
        div { class: "bd-root", style: "{accent_style}",

            // ── Hero ────────────────────────────────────────────────────
            section { class: "bd-hero",
                BdCrumb {
                    items: {
                        let mut crumbs = vec![("Home".to_string(), true)];
                        if !primary_author.is_empty() {
                            crumbs.push((primary_author.clone(), false));
                        }
                        crumbs.push((title.clone(), false));
                        crumbs
                    },
                }

                div { class: "bd-hero-grid",
                    // Cover column
                    div { class: "bd-cover-col",
                        Cover { book: b.clone() }
                        if !b.formats.is_empty() {
                            div { class: "bd-format-badges",
                                for f in b.formats.iter() {
                                    BdFormatBadge { fmt: f.clone() }
                                }
                            }
                        }
                    }

                    // Title + CTAs column
                    div { class: "bd-title-col",
                        div { class: "label", "{kicker}" }
                        div { class: "bd-title-row",
                            h1 { class: "bd-title", "{title}" }
                            Link {
                                to: Route::MetadataEdit { id: b.id },
                                class: "btn ghost sm bd-edit-hero",
                                "data-testid": "edit-metadata-hero",
                                title: "Edit metadata\u{2026}",
                                "aria-label": "Edit metadata",
                                span { class: "bd-ico-pencil" }
                                "Edit"
                            }
                        }
                        if !authors_line.is_empty() {
                            p {
                                class: "bd-by",
                                "data-testid": "book-authors",
                                "by "
                                span { class: "bd-author-link",
                                    "{authors_line}"
                                    span {
                                        class: "bd-author-link-hint",
                                        aria_hidden: "true",
                                        " view author \u{2192}"
                                    }
                                }
                            }
                        }
                        if let Some(desc) = b.description.as_deref() {
                            // Server-sanitized HTML — preserved from prior implementation.
                            div {
                                class: "bd-desc",
                                "data-testid": "book-description",
                                dangerous_inner_html: "{desc}",
                            }
                        }
                        div { class: "bd-cta-row",
                            if has_ebook {
                                button {
                                    class: "btn primary lg",
                                    disabled: true,
                                    title: "Reader coming soon",
                                    // TODO(F2.2): open the ebook reader, or "Resume" with progress (F2.1)
                                    "Start reading"
                                }
                            } else if has_audio {
                                button {
                                    class: "btn primary lg",
                                    disabled: true,
                                    title: "Audio player coming soon",
                                    // TODO(F2.3): open audiobook player
                                    "Start listening"
                                }
                            }
                            if has_audio && has_ebook {
                                button {
                                    class: "btn lg",
                                    disabled: true,
                                    title: "Audio player coming soon",
                                    // TODO(F2.3): open audiobook player
                                    "Listen"
                                }
                            }
                            button {
                                class: "btn lg ghost",
                                disabled: true,
                                title: "Send-to-Kindle coming soon",
                                // TODO(F4.3): send-to-kindle
                                "Send to Kindle"
                            }
                            button {
                                class: "btn lg ghost",
                                disabled: true,
                                title: "Send-to-Kobo coming soon",
                                // TODO(F4.1): send-to-kobo
                                "Send to Kobo"
                            }
                        }
                        // TODO(F2.1): replace stub progress with real reading-progress data
                        div { class: "bd-progress-meta", aria_hidden: "true",
                            div { class: "bd-progress-line",
                                span { class: "mono", "Not started" }
                                span { class: "mono", "0%" }
                            }
                            div { class: "pbar", i { style: "width: 0%;" } }
                        }
                        if !b.subjects.is_empty() {
                            ul { class: "bd-tag-list",
                                for tag in b.subjects.iter() {
                                    li { class: "chip", "{tag}" }
                                }
                            }
                        }
                    }

                    // Rating + actions card column
                    aside { class: "card bd-rating-card",
                        div { class: "label", "Your rating" }
                        // TODO(F3.2): wire up interactive rating
                        div { class: "bd-stars", aria_hidden: "true",
                            BdStars { value: 0.0 }
                        }
                        div { class: "mono bd-rating-meta", "Not rated yet" }

                        div { class: "divider" }

                        div { class: "label bd-action-head", "Actions" }
                        div { class: "bd-actions",
                            // TODO(F3.2): journal entry & highlight
                            button { class: "btn ghost bd-action-row", disabled: true,
                                span { "Write a journal entry" }
                                span { class: "bd-action-row-arrow", "\u{2192}" }
                            }
                            button { class: "btn ghost bd-action-row", disabled: true,
                                span { "Add a highlight" }
                                span { class: "bd-action-row-arrow", "\u{2192}" }
                            }
                            button { class: "btn ghost bd-action-row", disabled: true,
                                span { "Mark as finished" }
                                span { class: "bd-action-row-arrow", "\u{2192}" }
                            }
                            button { class: "btn ghost bd-action-row", disabled: true,
                                span { "Share or export\u{2026}" }
                                span { class: "bd-action-row-arrow", "\u{2192}" }
                            }
                        }

                        div { class: "divider" }

                        div { class: "label bd-shelves-head", "On your shelves" }
                        // TODO(F3.x): shelves / collections
                        div { class: "bd-shelves", aria_hidden: "true",
                            div { class: "chip", "Not on a shelf" }
                        }
                    }
                }
            }

            // ── Body ────────────────────────────────────────────────────
            section { class: "bd-body-grid",

                // Main column
                div { class: "bd-body-main",

                    // Journal — TODO(F3.2)
                    BdSectionHead { kicker: "Your journal · 0 entries".to_string(), title: "What you've written".to_string() }
                    div { class: "bd-journal-empty card", aria_hidden: "true",
                        p { class: "mono", "No journal entries yet." }
                        p { class: "bd-stub-hint", "Journaling lands in F3.2." }
                    }

                    div { class: "divider" }

                    // Highlights — TODO(F3.2)
                    BdSectionHead { kicker: "0 highlights".to_string(), title: "Passages you saved".to_string() }
                    div { class: "bd-journal-empty card", aria_hidden: "true",
                        p { class: "mono", "No highlights saved yet." }
                        p { class: "bd-stub-hint", "Highlights land in F3.2." }
                    }

                    div { class: "divider" }

                    // More by this author — TODO(F3.3)
                    BdSectionHead {
                        kicker: if primary_author.is_empty() { "More to read".to_string() } else { format!("More by {primary_author}") },
                        title: "From the same hand".to_string(),
                    }
                    div { class: "bd-stub-strip card", aria_hidden: "true",
                        p { class: "bd-stub-hint mono", "Author pages land in F3.3." }
                    }

                    div { class: "divider" }

                    // Suggested — TODO(F3.3)
                    BdSectionHead {
                        kicker: format!("If you liked {title}\u{2026}"),
                        title: "Suggested for you".to_string(),
                    }
                    div { class: "bd-stub-strip card", aria_hidden: "true",
                        p { class: "bd-stub-hint mono", "Suggestions land in F3.3." }
                    }
                }

                // Rail
                aside { class: "bd-rail",

                    // File details
                    div { class: "card",
                        div { class: "label bd-rail-head", "File details" }
                        table { class: "bd-meta-table mono",
                            tbody {
                                BdMetaRow { k: "Title".to_string(), v: title.clone() }
                                if !authors_line.is_empty() {
                                    BdMetaRow { k: "Author".to_string(), v: authors_line.clone() }
                                }
                                if let Some(p) = b.publisher.clone() {
                                    BdMetaRow { k: "Pub.".to_string(), v: p }
                                }
                                if let Some(d) = b.published.clone() {
                                    BdMetaRow { k: "Date".to_string(), v: d }
                                }
                                if let Some(l) = b.language.clone() {
                                    BdMetaRow { k: "Language".to_string(), v: l }
                                }
                                for ident in b.identifiers.iter() {
                                    BdMetaRow {
                                        k: ident.scheme.clone().unwrap_or_else(|| "ID".into()),
                                        v: ident.value.clone(),
                                    }
                                }
                            }
                        }

                        div { class: "divider" }

                        div { class: "label bd-rail-head", "Formats" }
                        // Relocated FormatSwitcher — same testids as before.
                        FormatSwitcher { formats: b.formats.clone() }

                        // F5.1: metadata editor
                        Link {
                            to: Route::MetadataEdit { id: b.id },
                            class: "btn ghost sm bd-rail-edit",
                            "data-testid": "edit-metadata",
                            "Edit metadata\u{2026}"
                        }
                    }

                    // Series / standalone
                    div { class: "card",
                        if let Some(s) = series_label.as_ref() {
                            div { class: "label bd-rail-head", "Series" }
                            p { class: "bd-rail-body", "{s}" }
                        } else {
                            div { class: "label bd-rail-head", "Standalone" }
                            p { class: "bd-rail-body", "Not part of a series." }
                        }
                    }

                    // Reading insights — TODO(F2.1)
                    div { class: "card",
                        div { class: "bd-insights-head",
                            div { class: "label", "Insights" }
                            span { class: "mono bd-insights-tag", "this book" }
                        }
                        div { class: "bd-insights-grid", aria_hidden: "true",
                            BdInsightCell { label: "Started".to_string(), value: "—".to_string() }
                            BdInsightCell { label: "Time read".to_string(), value: "—".to_string() }
                            BdInsightCell { label: "Sessions".to_string(), value: "—".to_string() }
                            BdInsightCell { label: "Pace".to_string(), value: "—".to_string() }
                        }
                        div { class: "divider" }
                        div { class: "label bd-rail-head", "Activity · last 22 days" }
                        // Placeholder sparkline — flat bars until F2.1 lands.
                        div { class: "bd-activity-bar", aria_hidden: "true",
                            for _ in 0..22u32 {
                                i { class: "bd-activity-tick" }
                            }
                        }
                        div { class: "bd-activity-axis mono",
                            span { "3wk ago" }
                            span { "minutes read · by day" }
                            span { "today" }
                        }
                    }
                }
            }

            // Footer affordance — Back to library link.
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

/// Atrium-styled breadcrumb. The first item is the "Home" link back to the
/// library; subsequent items are rendered as plain text segments.
#[component]
fn BdCrumb(items: Vec<(String, bool)>) -> Element {
    let last_idx = items.len().saturating_sub(1);
    rsx! {
        nav { class: "bd-crumb", "aria-label": "breadcrumb",
            for (i, (text, is_home)) in items.iter().cloned().enumerate() {
                if i > 0 {
                    span { class: "bd-crumb-sep", "\u{203a}" }
                }
                if is_home {
                    Link { to: Route::Landing {}, class: "bd-crumb-home", "{text}" }
                } else {
                    span {
                        class: if i == last_idx { "bd-crumb-curr" } else { "bd-crumb-step" },
                        "{text}"
                    }
                }
            }
        }
    }
}

/// Body section heading row — kicker label + serif title. The kicker stacks
/// above the title (mirrors `screens/_shared.jsx#SectionHead`).
#[component]
fn BdSectionHead(kicker: String, title: String) -> Element {
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
fn BdFormatBadge(fmt: String) -> Element {
    rsx! {
        div { class: "bd-fmt-badge", "{fmt}" }
    }
}

/// Read-only star display. Half-filled stars are rounded down to nearest
/// integer in the stub; F3.2 replaces this with the interactive widget.
#[component]
fn BdStars(value: f32) -> Element {
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
fn BdMetaRow(k: String, v: String) -> Element {
    rsx! {
        tr { class: "bd-meta-row",
            td { class: "bd-meta-k", "{k}" }
            td { class: "bd-meta-v", "{v}" }
        }
    }
}

#[component]
fn BdInsightCell(label: String, value: String) -> Element {
    rsx! {
        div { class: "bd-insight-cell",
            div { class: "mono bd-insight-label", "{label}" }
            div { class: "bd-insight-value", "{value}" }
        }
    }
}
