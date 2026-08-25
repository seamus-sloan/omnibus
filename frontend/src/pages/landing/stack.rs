//! The stack at the top of the web landing page: your open books fanned on a
//! centred stage, the fan spreading and tilting under the cursor, and the
//! front cover carrying the verb — the cover *is* the button. Plus
//! [`EdgeResume`], the ribbon that keeps resume reachable once the stack has
//! scrolled away. Both render only when a resume point exists, so SSR (empty
//! signal) and the first WASM paint agree — rule 07.

use dioxus::prelude::*;
use dioxus_router::{use_navigator, Link};
use omnibus_shared::{ProgressFormat, ResumePoint};

use super::resume_meta::resume_meta;
use crate::components::glyphs::{book_glyph, play_glyph};
use crate::Route;

/// Per-card tilt and lift, cycled so a fan of any length keeps the hand-dealt
/// look. Five entries covers `effects::HERO_POINTS`.
const FAN_ROT: [&str; 5] = ["-7deg", "-2.5deg", "3deg", "7.5deg", "11deg"];
const FAN_DY: [&str; 5] = ["0px", "6px", "3px", "10px", "7px"];

/// Whether the book carries both an ebook and an audiobook — the
/// dual-format gate for the link invitation (unlinked) and the Immersive
/// chip (linked).
fn has_both_formats(book: &omnibus_shared::EbookMetadata) -> bool {
    // Only the formats the indexer actually attaches (`db::audiobook`) —
    // matching the detail page's `has_audio`, so an invite here can never
    // point at a detail page with no sync affordance.
    book.formats.iter().any(|f| f.eq_ignore_ascii_case("epub"))
        && book
            .formats
            .iter()
            .any(|f| matches!(f.to_ascii_lowercase().as_str(), "m4b" | "m4a" | "mp3"))
}

/// The book+soundwave Immersive mark, accent stroked to read as the sync
/// feature's color.
fn immersive_mark() -> Element {
    rsx! {
        svg {
            width: "12", height: "12", view_box: "0 0 24 24", fill: "none",
            stroke: "var(--accent)", stroke_width: "1.9", stroke_linecap: "round",
            stroke_linejoin: "round", "aria-hidden": "true",
            path { d: "M4 5.2A2 2 0 0 1 6 4h4.2a1.8 1.8 0 0 1 1.8 1.8V18a1.6 1.6 0 0 0-1.6-1.6H6A2 2 0 0 1 4 14.4V5.2z" }
            path { d: "M15.5 8v8M18.5 6v12M21 9.5v5" }
        }
    }
}

/// A linked book's mapped "resume in the other format" candidate — the
/// route + label for the counterpart chip, or `None` when no cross-format
/// mapping exists. The chip routes to that surface, whose own prompt offers
/// the precise jump once loaded.
fn build_counterpart(point: &ResumePoint, uuid: &str) -> Option<(Route, String)> {
    point.cross_format.as_ref().map(|cf| match cf.target {
        ProgressFormat::Audio => (
            Route::BookListen {
                uuid: uuid.to_string(),
                file_id: cf.book_file_id,
            },
            format!(
                "Listen \u{00b7} \u{2248} {}",
                crate::components::alignment_modal::fmt_hm(
                    cf.audio_position_seconds.unwrap_or(0.0),
                ),
            ),
        ),
        ProgressFormat::Epub => (
            Route::BookRead {
                uuid: uuid.to_string(),
            },
            format!("Read \u{00b7} \u{2248} {}%", cf.percent.unwrap_or(0)),
        ),
    })
}

/// Pre-derived, ready-to-render values for one resume point — the
/// `PagerDisplay` pattern (`pages/comic_reader.rs`) applied here so the
/// component bodies read declaratively.
#[derive(Clone, PartialEq)]
pub(super) struct StackEntry {
    uuid: String,
    book: omnibus_shared::EbookMetadata,
    title: String,
    author: String,
    is_audio: bool,
    /// Whether the user has confirmed a cross-format link for this book —
    /// the kicker says so, since the stack has no per-card eyebrow to carry it.
    linked: bool,
    /// "Continue listening" / "Continue reading" / "Continue · synced".
    eyebrow: &'static str,
    /// Format-prefixed position line: "Audiobook · Ch. 14 · 68% · 2h 03m left".
    where_line: String,
    /// Lowercase verb + percent for the veil: "play · 68%".
    veil_label: String,
    pct: Option<i64>,
    accent_style: String,
    dual_unlinked: bool,
    dual_linked: bool,
    counterpart: Option<(Route, String)>,
    resume_route: Route,
    thumb_src: Option<String>,
    thumb_srcset: Option<String>,
}

impl StackEntry {
    fn from_point(point: &ResumePoint, server_url: &str, bust: CoverBust<'_>) -> Self {
        let uuid = point.record.book_uuid.clone();
        let book = point.book.clone();
        let title = book.title.as_deref().unwrap_or(&book.filename).to_string();
        let author = book
            .creators
            .first()
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let is_audio = point.record.format == ProgressFormat::Audio;
        let eyebrow = match (is_audio, point.linked) {
            (_, true) => "Continue \u{00b7} synced",
            (true, false) => "Continue listening",
            (false, false) => "Continue reading",
        };
        let (meta, pct) = resume_meta(point);
        let format_word = if is_audio { "Audiobook" } else { "Ebook" };
        let where_line = format!("{format_word} \u{00b7} {meta}");
        let verb = if is_audio { "play" } else { "resume" };
        let veil_label = match pct {
            Some(p) => format!("{verb} \u{00b7} {p}%"),
            None => verb.to_string(),
        };
        // The page and every card take the book's own accent, so the fan
        // reads as four different books rather than four tinted plates.
        let accent_style = book
            .accent
            .as_deref()
            .map(|a| format!("--accent: {a};"))
            .unwrap_or_default();
        let cover_bust = bust.get(&uuid).copied().unwrap_or(0);
        let (thumb_src, thumb_srcset) =
            crate::components::cover_tile::thumb_srcs(&book, &uuid, server_url, cover_bust);
        Self {
            uuid,
            title,
            author,
            is_audio,
            linked: point.linked,
            eyebrow,
            where_line,
            veil_label,
            pct,
            accent_style,
            // Dual-format but unlinked: the stack carries the link invitation
            // the design draws under the two-card state. Linked dual-format
            // books get the Immersive chip instead.
            dual_unlinked: !point.linked && has_both_formats(&book),
            dual_linked: point.linked && has_both_formats(&book),
            counterpart: build_counterpart(point, &point.record.book_uuid),
            // Shared dispatch (`routes::resume_route`) rather than a local
            // format match: it carries the audio point's `book_file_id` and
            // sends a CBZ-only book to the comic pager.
            resume_route: crate::routes::resume_route(point),
            book,
            thumb_src,
            thumb_srcset,
        }
    }
}

/// The cover cache-bust map, read out of context by the caller. Threaded in
/// as a plain map rather than read here so [`stack_entries`] stays a pure
/// derivation the unit tests can call without a Dioxus runtime.
type CoverBust<'a> = &'a std::collections::HashMap<String, u32>;

/// Derive one [`StackEntry`] per resume point, in the order the server
/// returned them (newest spot first).
pub(super) fn stack_entries(
    points: &[ResumePoint],
    server_url: &str,
    bust: CoverBust<'_>,
) -> Vec<StackEntry> {
    points
        .iter()
        .map(|p| StackEntry::from_point(p, server_url, bust))
        .collect()
}

/// The accent the whole page takes: the lead book's, falling back to the
/// Atrium default when it has none (or before the fetch lands).
pub(super) fn lead_accent_style(entries: &[StackEntry], lead: usize) -> String {
    entries
        .get(lead)
        .map(|e| e.accent_style.clone())
        .unwrap_or_default()
}

/// The stack: the lead book named in type on the left of nothing — centred —
/// with every open book fanned beneath it. Clicking the lead cover resumes;
/// clicking any other brings it forward, as do the arrow keys while a card
/// holds focus.
#[component]
pub(super) fn ResumeStack(entries: Vec<StackEntry>, mut lead: Signal<usize>) -> Element {
    let count = entries.len();
    let at = lead().min(count.saturating_sub(1));
    let Some(front) = entries.get(at).cloned() else {
        return rsx! {};
    };
    rsx! {
        section {
            class: "lmq-stack",
            "data-testid": "continue-stack",
            aria_label: "Continue reading",
            div { class: "lmq-stack-side",
                div { class: "lmq-kicker",
                    if count == 1 { "1 book open" } else { "{count} books open" }
                    if front.linked {
                        crate::components::sync_glyph::SyncGlyph { size: 13 }
                        "synced"
                    }
                }
                Link {
                    to: Route::BookDetail { uuid: front.uuid.clone() },
                    class: "lmq-title-link",
                    h2 { class: "lmq-title", "{front.title}" }
                }
                if !front.author.is_empty() {
                    div { class: "lmq-by", "{front.author}" }
                }
                div { class: "lmq-where", "{front.where_line}" }
                StackAlts { front: front.clone() }
                div { class: "lmq-stack-keys",
                    span { class: "lmq-key", kbd { "\u{21b5}" } "resume the front book" }
                    span { class: "lmq-key",
                        kbd { "\u{2190}" }
                        kbd { "\u{2192}" }
                        "bring another forward"
                    }
                }
            }
            div {
                class: "lmq-fan",
                // Arrow keys live on the fan, not on each card: `Link` takes
                // no key listener, and a document-level one would fight the
                // search box for the same keys. Events bubble here from
                // whichever card holds focus, so the on-screen hint is true.
                onkeydown: move |evt: Event<KeyboardData>| {
                    let step = match evt.key() {
                        Key::ArrowLeft => Some(count.saturating_sub(1)),
                        Key::ArrowRight => Some(1),
                        _ => None,
                    };
                    if let Some(step) = step {
                        evt.prevent_default();
                        let at = *lead.peek();
                        lead.set(at.saturating_add(step) % count.max(1));
                    }
                },
                for (i, entry) in entries.into_iter().enumerate() {
                    FanCard {
                        key: "{entry.uuid}",
                        entry,
                        index: i,
                        is_lead: i == at,
                        lead,
                    }
                }
            }
        }
    }
}

/// The lead book's cross-format affordances — Immersive, the counterpart
/// jump, and the "link these two" invitation — carried beside the type
/// rather than on the cover, which belongs to the resume verb alone.
#[component]
fn StackAlts(front: StackEntry) -> Element {
    let uuid = front.uuid.clone();
    let immersive_uuid = uuid.clone();
    rsx! {
        if front.dual_linked || front.counterpart.is_some() || front.dual_unlinked {
            div { class: "lmq-alts",
                if front.dual_linked {
                    button {
                        r#type: "button",
                        class: "lmq-alt",
                        "data-testid": "hero-immersive-{uuid}",
                        title: "Open the ereader and audiobook together, kept in sync",
                        onclick: move |_| {
                            crate::pages::retarget_and_open_immersive(immersive_uuid.clone())
                        },
                        {immersive_mark()}
                        span { "Immersive" }
                    }
                }
                if let Some((route, label)) = front.counterpart.clone() {
                    Link {
                        to: route,
                        class: "lmq-alt",
                        "data-testid": "hero-crossformat-{uuid}",
                        span { "{label}" }
                    }
                }
                if front.dual_unlinked {
                    Link {
                        to: Route::BookDetail { uuid: uuid.clone() },
                        class: "lmq-alt lmq-alt--invite",
                        "data-testid": "hero-link-invite-{uuid}",
                        crate::components::sync_glyph::SyncGlyph { size: 12 }
                        span { "Same book, two spots — link the formats." }
                    }
                }
            }
        }
    }
}

/// One cover in the fan. Every card is a `Link` on every target and in every
/// position — swapping element types between lead and non-lead would make the
/// diff replace the node and drop the handlers on its siblings (rule 07), and
/// the `href` is what keeps middle-click and open-in-new-tab working on the
/// resume affordance. The lead's click resumes; the rest suppress the
/// navigation (`onclick_only`) and bring themselves forward instead.
#[component]
fn FanCard(entry: StackEntry, index: usize, is_lead: bool, mut lead: Signal<usize>) -> Element {
    let StackEntry {
        uuid,
        book,
        title,
        is_audio,
        eyebrow,
        veil_label,
        pct,
        accent_style,
        resume_route,
        thumb_src,
        thumb_srcset,
        ..
    } = entry;
    // Front-to-back paint order with the lead always on top, so bringing a
    // card forward reads as it walking out of the pile.
    let z = if is_lead { 12 } else { 9 - index.min(9) };
    let style = format!(
        "{accent_style} --rot: {}; --dy: {}; --n: {index}; z-index: {z};",
        FAN_ROT[index % FAN_ROT.len()],
        FAN_DY[index % FAN_DY.len()],
    );
    let label = if is_lead {
        format!("{eyebrow}: {title}")
    } else {
        format!("Bring {title} forward")
    };
    let testid = format!("hero-resume-{uuid}");
    rsx! {
        // Every card is a `Link` to its own resume route, on every target and
        // in every position: swapping element types between lead and non-lead
        // would make the diff replace the node and drop the handlers on its
        // siblings (rule 07), and the href is what keeps middle-click and
        // open-in-new-tab working on the resume affordance. `onclick_only`
        // suppresses the navigation for the cards behind the front one, whose
        // click means "come forward" — the same thing the arrow keys do.
        Link {
            to: resume_route,
            onclick_only: !is_lead,
            onclick: move |_| lead.set(index),
            class: if is_lead { "lmq-fcard lead" } else { "lmq-fcard" },
            "data-testid": "{testid}",
            style: "{style}",
            title: "{title}",
            aria_label: "{label}",
            crate::components::atrium::Cover {
                book,
                src_override: thumb_src,
                srcset: thumb_srcset,
                sizes: Some("132px".to_string()),
            }
            if is_lead {
                span { class: "lmq-veil", aria_hidden: true,
                    span { class: "lmq-veil-mark",
                        if is_audio { {play_glyph(16)} } else { {book_glyph(16)} }
                    }
                    span { class: "lmq-veil-lab", "{veil_label}" }
                    if let Some(pct) = pct {
                        span { class: "lmq-veil-bar", i { style: "width:{pct}%" } }
                    }
                }
            }
        }
    }
}

/// Resume, once the stack has scrolled away: a ribbon caught in the page
/// edge rather than a bar across the top, which would read as the audiobook
/// mini-player. `marquee.js` raises `lmq--past` on the page root to reveal
/// it; hover (or tab) widens it into the control.
#[component]
pub(super) fn EdgeResume(entries: Vec<StackEntry>, lead: Signal<usize>) -> Element {
    let nav = use_navigator();
    let at = lead().min(entries.len().saturating_sub(1));
    let Some(entry) = entries.get(at).cloned() else {
        return rsx! {};
    };
    let StackEntry {
        uuid,
        book,
        title,
        is_audio,
        where_line,
        veil_label,
        accent_style,
        resume_route,
        thumb_src,
        thumb_srcset,
        ..
    } = entry;
    rsx! {
        div {
            class: "lmq-edge",
            "data-testid": "resume-edge",
            style: "{accent_style}",
            span { class: "lmq-edge-strip", aria_hidden: true,
                span { class: "lmq-edge-vert", "{veil_label}" }
            }
            span { class: "lmq-edge-card",
                span { class: "ec", aria_hidden: true,
                    crate::components::atrium::Cover {
                        book,
                        src_override: thumb_src,
                        srcset: thumb_srcset,
                        sizes: Some("40px".to_string()),
                    }
                }
                span { class: "et",
                    b { "{title}" }
                    span { "{where_line}" }
                }
                button {
                    r#type: "button",
                    class: "lmq-edge-go",
                    "data-testid": "resume-edge-go-{uuid}",
                    aria_label: "Resume {title}",
                    onclick: move |_| { nav.push(resume_route.clone()); },
                    if is_audio { {play_glyph(11)} } else { {book_glyph(11)} }
                    if is_audio { "Play" } else { "Resume" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
