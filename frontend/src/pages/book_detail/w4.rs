//! W4 "marquee" book-detail stage (web): the cover pinned huge on the left
//! under a soft parallax, content in a translucent panel on the right, and
//! seven snap-scrolled stops — Home · Shelf · Stats · Highlights · Journals ·
//! The files · Recommendations — with a hover-expanding dot-rail table of
//! contents. Scroll mechanics live in `w4.js` (installed post-mount, rule 07).

use dioxus::prelude::*;
use omnibus_shared::progress::ProgressFormat;
use omnibus_shared::{
    AlignmentView, BookInsights, EbookMetadata, ProgressRecord, SuggestionsResponse,
};

use crate::components::atrium::Cover;
use crate::data;

use super::body::{BdAuthorCluster, BdPageCtx, BdSameHand, BdSuggestionsStrip};
use super::highlights::{BdHighlightsSection, BdQuoteMeta};
use super::journal::W4JournalStop;
use super::view::LoadedBookView;
use super::{w4_files, w4_home, w4_shelf, w4_stats, PhysSignals};

/// The seven stops, in running order. The label pair is `NN` + name — the
/// section label renders `NN / 07 — Name` and the dot rail `NN · Name`.
pub(super) const W4_SECTIONS: [(&str, &str); 7] = [
    ("01", "Home"),
    ("02", "Shelf"),
    ("03", "Stats"),
    ("04", "Highlights"),
    ("05", "Journals"),
    ("06", "The files"),
    ("07", "Recommendations"),
];

/// Scroll-mechanics glue (parallax, dot tracking, dot-rail navigation).
const W4_JS: &str = include_str!("w4.js");

/// Everything the stage needs beyond the book itself, bundled to keep the
/// component under the prop-count guideline (mirrors `view::LoadedCtx`).
#[derive(Clone, PartialEq, Props)]
pub(super) struct W4StageCtx {
    pub server_url: String,
    pub is_admin: bool,
    pub refresh: Signal<u32>,
    pub after_merge: Signal<bool>,
}

/// Prebuilt admin rail actions (Merge / Delete dialogs' openers), threaded
/// into the files stop.
#[derive(Clone, PartialEq, Props)]
pub(super) struct W4AdminActions {
    pub merge_button: Option<Element>,
    pub delete_button: Option<Element>,
}

/// The full W4 stage. Owns the page-wide fetches the stops share (reading /
/// listening progress and per-book insights) plus the post-mount scroll glue;
/// each stop fetches its own section data (series, shelves, journal,
/// highlights, physical copies) exactly as the sections it absorbed did.
#[component]
pub(super) fn W4Stage(
    b: EbookMetadata,
    view: W4ViewFacts,
    author_books: Vec<EbookMetadata>,
    suggestions: Option<SuggestionsResponse>,
    admin: W4AdminActions,
    phys: PhysSignals,
    ctx: W4StageCtx,
) -> Element {
    let uuid = b.unique_identifier.clone().unwrap_or_default();

    // Page-wide data: newest saved position per format + session insights.
    // Seeded `None` on SSR and the first WASM paint (rule 07); post-mount
    // effects fill them in. A monotonic sequence guards against a slow fetch
    // for a previous book landing after a fast SPA navigation.
    let mut reading = use_signal(|| None::<ProgressRecord>);
    let mut listening = use_signal(|| None::<ProgressRecord>);
    let mut insights = use_signal(|| None::<BookInsights>);
    // The alignment view doubles as the chapter table + audio timeline for
    // the Home stop's CTA labels and ruler ticks — fetched for any book with
    // files, not just dual-format ones.
    let mut alignment = use_signal(|| None::<AlignmentView>);
    let mut load_seq = use_signal(|| 0u64);
    {
        let uuid = uuid.clone();
        let has_text = view.has_ebook || view.has_comic;
        let has_audio = view.has_audio;
        let refresh = ctx.refresh;
        use_effect(use_reactive!(|uuid| {
            let _ = refresh();
            let my_load = *load_seq.peek() + 1;
            load_seq.set(my_load);
            reading.set(None);
            listening.set(None);
            insights.set(None);
            alignment.set(None);
            let uuid = uuid.clone();
            spawn(async move {
                let read = if has_text {
                    data::get_progress("", &uuid, ProgressFormat::Epub)
                        .await
                        .ok()
                        .flatten()
                } else {
                    None
                };
                let listen = if has_audio {
                    data::get_progress("", &uuid, ProgressFormat::Audio)
                        .await
                        .ok()
                        .flatten()
                } else {
                    None
                };
                let ins = data::get_book_insights(&uuid).await.ok().flatten();
                let align = if has_text || has_audio {
                    data::get_alignment("", &uuid).await.ok()
                } else {
                    None
                };
                if *load_seq.peek() == my_load {
                    reading.set(read);
                    listening.set(listen);
                    insights.set(ins);
                    alignment.set(align);
                }
            });
        }));
    }

    // Install the scroll glue after mount and re-run it when the book
    // changes (resets to stop 01, replaces listeners). Web-only interop —
    // the eval is a no-op on SSR.
    {
        let uuid = uuid.clone();
        use_effect(use_reactive!(|uuid| {
            let _ = uuid;
            let _ = dioxus::document::eval(W4_JS);
        }));
    }

    // Wishlist-only books (no files, tracked entry) override the Home /
    // Stats / Highlights / Journals stops with the design's empty states.
    // The entry arrives post-mount via the shared `phys` signals, so SSR and
    // the first client paint both render the default stops (rule 07).
    let is_fileless = b.formats.is_empty();
    let wishlist = phys.wishlist.read().clone();
    let wish_mode = is_fileless && wishlist.is_some();

    let quote_meta = BdQuoteMeta {
        title: view.title.clone(),
        author: view.primary_author.clone(),
        accent: b.accent.clone(),
    };

    let stops: [Element; 7] = [
        rsx! {
            w4_home::W4HomeStop {
                b: b.clone(),
                view: view.clone(),
                progress: W4Progress { reading: reading(), listening: listening() },
                insights: insights(),
                alignment: alignment(),
                phys,
                refresh: ctx.refresh,
                after_merge: ctx.after_merge,
                wishlist: wishlist.clone(),
            }
        },
        rsx! {
            w4_shelf::W4ShelfStop { b: b.clone(), view: view.clone() }
        },
        rsx! {
            w4_stats::W4StatsStop {
                uuid: uuid.clone(),
                insights: insights(),
                progress: W4Progress { reading: reading(), listening: listening() },
                audio_only: view.has_audio && !view.has_ebook && !view.has_comic,
                wish_mode,
            }
        },
        rsx! {
            if wish_mode {
                div { class: "bdw4-k", "Kept lines \u{b7} none" }
                div { class: "bdw4-bigquiet", "Highlight while you read to keep lines here." }
                p { class: "mono bdw4-quiet-hint", "find a copy first \u{2014} check in when it arrives" }
            } else {
                BdHighlightsSection { uuid: uuid.clone(), quote_meta }
            }
        },
        rsx! {
            W4JournalStop { uuid: uuid.clone(), wish_mode }
        },
        rsx! {
            w4_files::W4FilesStop {
                b: b.clone(),
                view: view.clone(),
                admin: admin.clone(),
                phys,
                refresh: ctx.refresh,
                is_fileless,
            }
        },
        rsx! {
            div { class: "bdw4-tight",
                BdSameHand {
                    author: BdAuthorCluster {
                        primary_author: view.primary_author.clone(),
                        author_id: view.author_id,
                        author_books: author_books.clone(),
                    },
                }
                BdSuggestionsStrip {
                    book_title: view.title.clone(),
                    suggestions,
                    ctx: BdPageCtx {
                        server_url: ctx.server_url.clone(),
                        is_admin: ctx.is_admin,
                    },
                }
            }
        },
    ];

    rsx! {
        div { class: "bdw4-stage", id: "bdw4-stage",
            div { class: "bdw4-coverwrap", aria_hidden: "true",
                div { class: "bdw4-coverpx", id: "bdw4-coverpx",
                    Cover { book: b.clone() }
                }
            }
            nav { class: "bdw4-dots", "aria-label": "Page sections", "data-testid": "bdw4-dots",
                for (i, (no, name)) in W4_SECTIONS.iter().enumerate() {
                    button {
                        key: "{no}",
                        r#type: "button",
                        class: if i == 0 { "bdw4-dotrow on" } else { "bdw4-dotrow" },
                        title: "{name}",
                        "data-testid": "bdw4-dot-{i}",
                        i {}
                        span { class: "lb", "{no} \u{b7} {name}" }
                    }
                }
            }
            div { class: "bdw4-snap", id: "bdw4-snap",
                for (i, (no, name)) in W4_SECTIONS.iter().enumerate() {
                    section {
                        key: "{no}",
                        class: "bdw4-sec",
                        "data-testid": "bdw4-sec-{i}",
                        div { class: "bdw4-seclab", "{no} / 07 \u{2014} " b { "{name}" } }
                        div { class: "bdw4-panel",
                            div { class: "bdw4-panel-inner", {stops[i].clone()} }
                        }
                        if i < W4_SECTIONS.len() - 1 {
                            div { class: "bdw4-next", aria_hidden: "true",
                                "scroll \u{2014} next: {W4_SECTIONS[i + 1].1}"
                                span { class: "arr", "\u{2193}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Display facts shared by every stop, derived once in `view.rs` from the
/// loaded book. Cloneable so each stop takes its own copy.
#[derive(Clone, PartialEq)]
pub(super) struct W4ViewFacts {
    pub title: String,
    pub primary_author: String,
    pub author_id: Option<i64>,
    pub authors_line: String,
    pub series: Option<String>,
    pub has_ebook: bool,
    pub has_audio: bool,
    pub has_comic: bool,
}

impl W4ViewFacts {
    pub(super) fn from_loaded(v: &LoadedBookView) -> Self {
        Self {
            title: v.title.clone(),
            primary_author: v.primary_author.clone(),
            author_id: v.author_id,
            authors_line: v.authors_line.clone(),
            series: v.series.clone(),
            has_ebook: v.has_ebook,
            has_audio: v.has_audio,
            has_comic: v.has_comic,
        }
    }
}

/// The newest saved position per format, threaded into the Home and Stats
/// stops. Either side is `None` until the post-mount fetch resolves — or
/// forever, for a format the reader has never opened.
#[derive(Clone, PartialEq)]
pub(super) struct W4Progress {
    pub reading: Option<ProgressRecord>,
    pub listening: Option<ProgressRecord>,
}

impl W4Progress {
    /// Whole-book percent from the freshest format that carries one.
    pub(super) fn newest_percent(&self) -> Option<i64> {
        let read = self
            .reading
            .as_ref()
            .and_then(|r| r.progress_percent.map(|p| (r.client_updated_at, p)));
        let listen = self
            .listening
            .as_ref()
            .and_then(|l| l.progress_percent.map(|p| (l.client_updated_at, p)));
        match (read, listen) {
            (Some((rt, rp)), Some((lt, lp))) => Some(if lt > rt { lp } else { rp }),
            (Some((_, p)), None) | (None, Some((_, p))) => Some(p),
            (None, None) => None,
        }
    }

    /// Unix seconds of the freshest saved position, if any.
    pub(super) fn newest_updated_at(&self) -> Option<i64> {
        let read = self.reading.as_ref().map(|r| r.client_updated_at);
        let listen = self.listening.as_ref().map(|l| l.client_updated_at);
        read.into_iter().chain(listen).max()
    }
}
