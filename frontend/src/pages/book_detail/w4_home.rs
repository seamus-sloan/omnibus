//! Stop 01 · Home — the W4 hero panel: kicker, marquee title, authors,
//! genre/tag chips, description, the read/listen/immersive/export CTA row,
//! reading-status control, and the position ruler with the sync line under
//! it. Wishlist-only books swap the CTA row for the design's
//! find-a-copy / check-in / remove actions.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::physical::WishlistEntry;
use omnibus_shared::summary::summary_is_sparse;
use omnibus_shared::{AlignmentView, BookFileInfo, BookInsights, EbookMetadata, MetadataOverrides};

use crate::components::alignment_modal::{fmt_hm, listening_frac};

use crate::components::FetchSummaryButton;
use crate::{data, use_server_url, Route};

use super::chips::{BdChipKind, BdChipListEditor};
use super::dates::{fmt_long_date, local_date_offset, use_local_dates_ready};
use super::export_menu::{BdExportContext, BdExportMenu};
use super::file_picker::{is_audio_book_file, BdFilePickerMenu, FilePickerChrome, FilePickerKind};
use super::immersive::BdImmersiveButton;
use super::physical::{find_a_copy_url, remove_from_wishlist};
use super::read_status::BdReadStatusControl;
use super::sync_link::BdSyncPanel;
use super::w4::{W4Progress, W4ViewFacts};
use super::PhysSignals;

/// The Home stop.
#[component]
pub(super) fn W4HomeStop(
    b: EbookMetadata,
    view: W4ViewFacts,
    progress: W4Progress,
    insights: Option<BookInsights>,
    alignment: Option<AlignmentView>,
    /// How many books of this series the library holds — the kicker's
    /// "Book N of M". `None` until the stage's series fetch resolves.
    series_total: Option<usize>,
    phys: PhysSignals,
    refresh: Signal<u32>,
    after_merge: Signal<bool>,
    wishlist: Option<WishlistEntry>,
) -> Element {
    let uuid = b.unique_identifier.clone().unwrap_or_default();
    let is_fileless = b.formats.is_empty();
    let wish_mode = is_fileless && wishlist.is_some();

    // Local copy of the effective summary so Fetch Summary can refresh the
    // shown description in place (same pattern the old hero used). Seeded
    // identically on SSR and first WASM paint (rule 07).
    let mut description = use_signal(|| b.description.clone().unwrap_or_default());
    let server_url = use_server_url();
    let save_uuid = uuid.clone();
    let on_fetched = move |text: String| {
        let url = server_url.clone();
        let uuid = save_uuid.clone();
        spawn(async move {
            let overrides = MetadataOverrides {
                description: Some(text.clone()),
                ..Default::default()
            };
            if data::save_overrides(&url, &uuid, &overrides).await.is_ok() {
                description.set(text);
            }
        });
    };

    let dates_ready = use_local_dates_ready();

    let kicker = home_kicker(&b, &view, series_total, wish_mode, wishlist.as_ref());
    let dual = view.has_ebook && view.has_audio;
    let readout = derive_readout(alignment.as_ref(), &progress);

    rsx! {
        div { class: "bdw4-k",
            if let (Some(label), Some(sid)) = (kicker.series_label.clone(), b.series_id) {
                Link { to: Route::SeriesDetail { id: sid }, class: "bdw4-k-link", "{label}" }
                "{kicker.tail}"
            } else {
                "{kicker.text}"
            }
        }
        div { class: "bdw4-titlerow",
            h1 { class: "bdw4-title", "{view.title}" }
        }
        Link {
            to: Route::MetadataEdit { uuid: uuid.clone() },
            class: "btn ghost sm bdw4-editcorner",
            "data-testid": "edit-metadata-hero",
            title: "Edit metadata\u{2026}",
            "aria-label": "Edit metadata",
            "\u{270e} Edit"
        }
        if !b.creators.is_empty() {
            p { class: "bdw4-by", "data-testid": "book-authors",
                "by "
                for (i, creator) in b.creators.iter().enumerate() {
                    if i > 0 { ", " }
                    if let Some(author_id) = creator.id {
                        Link {
                            key: "id-{author_id}",
                            to: Route::AuthorDetail { id: author_id },
                            class: "bd-author-link",
                            "{creator.name}"
                        }
                    } else {
                        span {
                            key: "name-{creator.name}-{creator.role:?}-{creator.file_as:?}",
                            class: "bd-author-link",
                            "{creator.name}"
                        }
                    }
                }
            }
        }
        div { class: "bdw4-chips",
            BdChipListEditor {
                uuid: uuid.clone(),
                kind: BdChipKind::Genres,
                values: b.genres.clone(),
            }
            BdChipListEditor {
                uuid: uuid.clone(),
                kind: BdChipKind::Tags,
                values: b.subjects.clone(),
            }
        }
        if !description().is_empty() {
            div { class: "bdw4-desc bd-desc", "data-testid": "book-description", dangerous_inner_html: "{description()}" }
        }
        if summary_is_sparse(&description()) {
            FetchSummaryButton { uuid: uuid.clone(), on_fetched }
        }
        if wish_mode {
            W4WishlistCtas {
                uuid: uuid.clone(),
                isbn: b.isbn13.clone(),
                view: view.clone(),
                phys,
            }
        } else {
            W4CtaRow {
                b: b.clone(),
                view: view.clone(),
                progress: progress.clone(),
                readout: readout.clone(),
            }
        }
        div { class: "bdw4-statusrow",
            BdReadStatusControl { uuid: uuid.clone() }
            if let Some(w) = wishlist.as_ref() {
                span { class: "mono bdw4-statusnote",
                    "tracking since {fmt_long_date(w.added_at, local_date_offset(dates_ready(), w.added_at))}"
                }
            }
        }
        if !wish_mode {
            {render_ruler(&progress, &readout, insights.as_ref(), dates_ready(), !dual)}
            if dual {
                BdSyncPanel { uuid: uuid.clone(), refresh, after_merge, w4: true }
            }
        }
    }
}

/// Chapter- and timeline-aware display facts derived from the alignment
/// view: what the design's CTA labels, ruler ticks, and caret flag show.
#[derive(Clone, PartialEq, Default)]
pub(super) struct HomeReadout {
    /// Total ebook chapters, for the ruler's tick count.
    pub ch_total: Option<usize>,
    /// 1-based current chapter + its title, from the reading percent against
    /// the chapter table.
    pub ch_now: Option<(usize, String)>,
    /// Listening position as `Hh Mm` on the whole audio timeline.
    pub audio_at: Option<String>,
    /// Whole-timeline audio duration as `Hh Mm`.
    pub audio_total: Option<String>,
    /// Seconds left on the audio timeline at 1.0×, for the pace estimate.
    pub audio_left_secs: Option<i64>,
}

/// Chapter titles come from the EPUB's own nav, where they run anywhere from
/// "Cinder" to "CHAPTER XIII DR. SEWARD'S DIARY—continued". The CTA names the
/// chapter inline, so cap it at a phrase rather than letting one book's
/// verbose nav stretch the button across the panel.
fn short_chapter_title(title: &str) -> String {
    const CAP: usize = 32;
    let t = title.trim();
    if t.chars().count() <= CAP {
        return t.to_string();
    }
    // Prefer breaking at the last word boundary inside the cap so the label
    // ends on a whole word.
    let head: String = t.chars().take(CAP).collect();
    let cut = head.rfind(char::is_whitespace).unwrap_or(head.len());
    format!(
        "{}\u{2026}",
        head[..cut].trim_end_matches(['—', '-', ',', ':'])
    )
}

fn derive_readout(alignment: Option<&AlignmentView>, progress: &W4Progress) -> HomeReadout {
    let Some(v) = alignment else {
        return HomeReadout::default();
    };
    let chapters = v.ebook.as_ref().map(|e| &e.chapters);
    let ch_total = chapters.map(|c| c.len()).filter(|n| *n > 1);
    let ch_now = match (
        chapters,
        progress.reading.as_ref().and_then(|r| r.progress_percent),
    ) {
        (Some(chapters), Some(pct)) if !chapters.is_empty() && pct > 0 => {
            // Chapter starts are whole-book percents (0..=100), same scale
            // as the saved position.
            let idx = chapters
                .iter()
                .rposition(|c| c.percent <= pct as f64)
                .unwrap_or(0);
            Some((idx + 1, short_chapter_title(&chapters[idx].title)))
        }
        _ => None,
    };
    let total_secs: f64 = v.audio_files.iter().map(|f| f.duration_seconds).sum();
    let audio_total = (total_secs > 0.0).then(|| fmt_hm(total_secs));
    let files: Vec<_> = v.audio_files.iter().collect();
    let frac = listening_frac(v, &files);
    let audio_at = frac.map(|f| fmt_hm(f * total_secs));
    let audio_left_secs = frac
        .filter(|_| total_secs > 0.0)
        .map(|f| ((1.0 - f) * total_secs) as i64);
    HomeReadout {
        ch_total,
        ch_now,
        audio_at,
        audio_total,
        audio_left_secs,
    }
}

/// The kicker line's parts: an optional series link + trailing text, or one
/// plain string.
struct HomeKicker {
    text: String,
    series_label: Option<String>,
    tail: String,
}

/// `Series · Book N · YYYY` (series linked) / `Genre · standalone · YYYY` /
/// `On your wishlist · <source>`.
fn home_kicker(
    b: &EbookMetadata,
    view: &W4ViewFacts,
    series_total: Option<usize>,
    wish_mode: bool,
    wishlist: Option<&WishlistEntry>,
) -> HomeKicker {
    if wish_mode {
        let source = wishlist
            .map(|w| super::physical::source_label(w.source))
            .unwrap_or("your wishlist");
        return HomeKicker {
            text: format!("On your wishlist \u{b7} added from {source}"),
            series_label: None,
            tail: String::new(),
        };
    }
    let year = b
        .published
        .as_deref()
        .and_then(|p| p.get(0..4))
        .unwrap_or("")
        .to_string();
    let year_tail = if year.is_empty() {
        String::new()
    } else {
        format!(" \u{b7} {year}")
    };
    // The bare series name, not the `Name #N` label: the position follows it
    // as its own "Book N of M" clause, per the design.
    if let Some(series) = b.series.clone().or_else(|| view.series.clone()) {
        // The design's "Kingkiller Chronicle · Book 1 of 3 · 2007": the
        // position comes from the book's own series index, the total from
        // how many of the series the library holds.
        let position = match (b.series_index.as_deref(), series_total) {
            (Some(n), Some(total)) if total > 0 => format!(" \u{b7} Book {n} of {total}"),
            (Some(n), _) => format!(" \u{b7} Book {n}"),
            (None, _) => String::new(),
        };
        let tail = format!("{position}{year_tail}");
        return HomeKicker {
            text: format!("{series}{tail}"),
            series_label: Some(series),
            tail,
        };
    }
    // The design leads a standalone with its category; genres are ours, with
    // a scanned subject as the fallback before the generic word.
    let lead = b
        .genres
        .first()
        .or_else(|| b.subjects.first())
        .cloned()
        .unwrap_or_else(|| "Book".to_string());
    HomeKicker {
        text: format!("{lead} \u{b7} standalone{year_tail}"),
        series_label: None,
        tail: String::new(),
    }
}

/// The CTA row: primary read/listen picker, secondary listen, Immersive, and
/// the Export menu — the old hero's `BdCtaRow` re-voiced for W4 (resume verbs
/// when a position exists; identical testids).
#[component]
fn W4CtaRow(
    b: EbookMetadata,
    view: W4ViewFacts,
    progress: W4Progress,
    #[props(default)] readout: HomeReadout,
) -> Element {
    let uuid = b.unique_identifier.clone().unwrap_or_default();
    let book_author = b
        .creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let title = view.title.clone();
    let epub_files: Vec<BookFileInfo> = b
        .book_files
        .iter()
        .filter(|f| f.format.eq_ignore_ascii_case("EPUB"))
        .cloned()
        .collect();
    let audio_files: Vec<BookFileInfo> = b
        .book_files
        .iter()
        .filter(|f| is_audio_book_file(f))
        .cloned()
        .collect();
    let started_reading = progress
        .reading
        .as_ref()
        .and_then(|r| r.progress_percent)
        .is_some_and(|p| p > 0);
    let started_listening = progress
        .listening
        .as_ref()
        .map(|l| l.audio_position_seconds.unwrap_or_default() > 0.0)
        .unwrap_or(false);
    // Design voice: the resume CTA names the chapter when the chapter table
    // is known — "Resume — Ch. 41 · Cinder" — and the secondary listen names
    // the timeline position — "Listen from 15h 21m".
    let read_verb = match (&readout.ch_now, started_reading) {
        (Some((n, title)), true) => format!("Resume \u{2014} Ch. {n} \u{b7} {title}"),
        (None, true) => "Resume reading".to_string(),
        (_, false) => "Start reading".to_string(),
    };
    let listen_primary = match (&readout.audio_at, started_listening) {
        (Some(at), true) => format!("Resume listening from {at}"),
        (None, true) => "Resume listening".to_string(),
        (_, false) => "Start listening".to_string(),
    };
    let listen_secondary = match (&readout.audio_at, started_listening) {
        (Some(at), true) => format!("Listen from {at}"),
        _ => "Listen".to_string(),
    };
    let is_fileless = !view.has_ebook && !view.has_audio && !view.has_comic;

    rsx! {
        div { class: "bdw4-ctarow",
            if is_fileless {
                p { class: "bd-no-files", "data-testid": "no-files-disclaimer",
                    "No ebook or audiobook files in your library yet \u{2014} this title is tracked from your physical collection or wishlist."
                }
            } else {
                if view.has_ebook {
                    BdFilePickerMenu {
                        uuid: uuid.clone(),
                        kind: FilePickerKind::Read,
                        files: epub_files.clone(),
                        chrome: FilePickerChrome {
                            label: read_verb.clone(),
                            button_class: "btn primary lg".to_string(),
                            single_testid: "start-reading".to_string(),
                        },
                    }
                } else if view.has_comic {
                    Link {
                        to: Route::ComicRead { uuid: uuid.clone() },
                        class: "btn primary lg",
                        "data-testid": "start-reading-comic",
                        "{read_verb}"
                    }
                } else if view.has_audio {
                    BdFilePickerMenu {
                        uuid: uuid.clone(),
                        kind: FilePickerKind::Listen,
                        files: audio_files.clone(),
                        chrome: FilePickerChrome {
                            label: listen_primary.clone(),
                            button_class: "btn primary lg".to_string(),
                            single_testid: "start-listening".to_string(),
                        },
                    }
                }
                if view.has_audio && (view.has_ebook || view.has_comic) {
                    BdFilePickerMenu {
                        uuid: uuid.clone(),
                        kind: FilePickerKind::Listen,
                        files: audio_files.clone(),
                        chrome: FilePickerChrome {
                            label: listen_secondary.clone(),
                            button_class: "btn lg".to_string(),
                            single_testid: "listen-secondary".to_string(),
                        },
                    }
                }
                if view.has_audio && view.has_ebook {
                    BdImmersiveButton { uuid: uuid.clone(), label: "Immersive" }
                }
                BdExportMenu {
                    ctx: BdExportContext {
                        uuid: uuid.clone(),
                        has_ebook: view.has_ebook,
                        has_audio: view.has_audio,
                        book_author: book_author.clone(),
                        book_title: title.clone(),
                        epub_size_bytes: b.epub_size_bytes,
                    },
                }
            }
        }
    }
}

/// Wishlist-only CTA row: Find a copy · Check in when acquired · Remove from
/// wishlist (danger on hover, per the design). A real component — it mounts
/// conditionally, so it must own its hooks in its own scope.
#[component]
fn W4WishlistCtas(
    uuid: String,
    isbn: Option<String>,
    view: W4ViewFacts,
    phys: PhysSignals,
) -> Element {
    let find_url = find_a_copy_url(isbn.as_deref(), &view.title, &view.primary_author);
    let server_url = use_server_url();
    let mut check_in_open = use_context::<crate::pages::CheckInOpen>().0;
    let wishlist = phys.wishlist;
    let busy = use_signal(|| false);
    let err = use_signal(|| None::<String>);
    rsx! {
        div { class: "bdw4-ctarow",
            a {
                class: "btn primary lg",
                "data-testid": "find-a-copy",
                href: "{find_url}",
                target: "_blank",
                rel: "noopener noreferrer",
                "Find a copy"
            }
            button {
                class: "btn lg",
                "data-testid": "wishlist-check-in",
                onclick: move |_| check_in_open.set(true),
                "Check in when acquired"
            }
            button {
                class: "btn lg ghost bdw4-danger",
                "data-testid": "wishlist-remove",
                disabled: busy(),
                onclick: move |_| {
                    remove_from_wishlist(wishlist, busy, err, server_url.clone(), uuid.clone())
                },
                "Remove from wishlist"
            }
        }
        if let Some(e) = err.read().clone() {
            p { role: "alert", class: "bd-phys-error", "data-testid": "wishlist-error", "{e}" }
        }
    }
}

/// The position ruler + the mono line under it, per the design: chapter
/// ticks when the chapter table is known, a caret flag naming the chapter
/// (or the bare percent), the `Ch. N of M · pct%` axis, and a time-left
/// estimate — the audio timeline at 1.0× when one exists, else scaled from
/// this book's recorded pace.
fn render_ruler(
    progress: &W4Progress,
    readout: &HomeReadout,
    insights: Option<&BookInsights>,
    dates_ready: bool,
    // Dual-format books get the sync line instead of a second recency line.
    show_last_opened: bool,
) -> Element {
    let Some(pct) = progress.newest_percent() else {
        return rsx! {};
    };
    let pct = pct.clamp(0, 100);
    let done = pct >= 100;
    let flag = match &readout.ch_now {
        Some((n, _)) => format!("Ch. {n}"),
        None => format!("{pct}%"),
    };
    let left_label = match (&readout.ch_now, readout.ch_total) {
        (Some((n, _)), Some(total)) => format!("Ch. {n} of {total} \u{b7} {pct}%"),
        _ => match (&readout.audio_at, &readout.audio_total) {
            (Some(at), Some(of)) => format!("{at} of {of} \u{b7} {pct}%"),
            _ => format!("{pct}% of the book"),
        },
    };
    let right_label = if done {
        None
    } else if let Some(left) = readout.audio_left_secs.filter(|l| *l > 0) {
        Some(format!(
            "\u{2248} {} left at 1.0\u{d7}",
            super::w4_stats::duration_label(left)
        ))
    } else {
        insights.filter(|_| pct > 0).map(|i| {
            let left_secs = i.seconds_total * (100 - pct) / pct;
            format!(
                "\u{2248} {} left at your pace",
                super::w4_stats::duration_label(left_secs)
            )
        })
    };
    let when = progress
        .newest_updated_at()
        .filter(|_| show_last_opened)
        .map(|t| {
            format!(
                "last opened {}",
                fmt_long_date(t, local_date_offset(dates_ready, t))
            )
        });
    let tick_style = readout
        .ch_total
        .map(|n| format!("--n:{n};"))
        .unwrap_or_default();
    rsx! {
        div { class: "bdw4-rulerwrap", "data-testid": "bdw4-ruler",
            div { class: "rx-ruler",
                div {
                    class: if done { "rx-fill done" } else { "rx-fill" },
                    style: "width:{pct}%",
                }
                if readout.ch_total.is_some() {
                    div { class: "rx-tix", style: "{tick_style}" }
                }
                if !done {
                    div { class: "rx-caret", style: "left:{pct}%", i { "{flag}" } }
                }
            }
            div { class: "rx-ruler-axis",
                span { "{left_label}" }
                if let Some(r) = right_label {
                    span { "{r}" }
                }
            }
            if let Some(w) = when {
                div { class: "mono bdw4-lastopened", "{w}" }
            }
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests;
