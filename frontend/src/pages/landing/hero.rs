//! Continue-reading hero carousel at the top of the web landing page: up to
//! [`super::effects::HERO_POINTS`] in-progress books as full-width cards in a
//! scroll-snap track with paging dots, mirroring the iOS `ContinueHero`.
//! Rendered only when at least one resume point exists, so SSR (empty signal)
//! and the first WASM paint agree — rule 07.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{ProgressFormat, ResumePoint};

use super::resume_meta::resume_meta;
use crate::components::glyphs::{book_glyph, play_glyph};
use crate::Route;

/// Watch the hero cards and report the index of the mostly-visible one, so
/// the dots track manual swipes/scrolls. Waits briefly for the track to exist
/// (the eval can run before the DOM patch lands), then observes each card.
#[cfg(feature = "web")]
const HERO_SYNC_JS: &str = r#"
(async function(){
  function track(){ return document.querySelector('[data-testid="continue-hero-track"]'); }
  var appear = Date.now() + 1000;
  while (!track() && Date.now() < appear) {
    await new Promise(function(r){ requestAnimationFrame(r); });
  }
  var t = track();
  if (!t) { return; }
  if (window.__omnibusHeroObs) { window.__omnibusHeroObs.disconnect(); }
  var cards = Array.prototype.slice.call(t.querySelectorAll('.ch-card'));
  var obs = new IntersectionObserver(function(entries){
    entries.forEach(function(e){
      if (e.isIntersecting) {
        try { dioxus.send(cards.indexOf(e.target)); } catch (_err) {}
      }
    });
  }, { root: t, threshold: 0.6 });
  cards.forEach(function(c){ obs.observe(c); });
  window.__omnibusHeroObs = obs;
})();
"#;

/// Smooth-scroll the track to card `index` (instant under reduced motion).
#[cfg(feature = "web")]
fn scroll_track_to(index: usize) {
    let js = format!(
        r#"
        var t = document.querySelector('[data-testid="continue-hero-track"]');
        if (t) {{
            var el = t.querySelectorAll('.ch-card')[{index}];
            if (el) {{
                var reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
                t.scrollTo({{ left: el.offsetLeft - t.offsetLeft, behavior: reduce ? 'auto' : 'smooth' }});
            }}
        }}
        "#
    );
    let _ = dioxus::document::eval(&js);
}

#[cfg(not(feature = "web"))]
fn scroll_track_to(_index: usize) {}

/// Arm the manual-scroll → dot sync channel. Web-only interop, gated at the
/// function definition so the render path stays unconditional (rule 07).
#[cfg(feature = "web")]
fn use_hero_dot_sync(mut page: Signal<usize>) {
    let mut sync_eval = use_hook(|| dioxus::document::eval(HERO_SYNC_JS));
    use_future(move || async move {
        while let Ok(idx) = sync_eval.recv::<i32>().await {
            if idx >= 0 {
                page.set(idx as usize);
            }
        }
    });
}

#[cfg(not(feature = "web"))]
fn use_hero_dot_sync(_page: Signal<usize>) {
    // Keep hook parity with the web variant: declare the same hooks so the
    // server render walks an identical hook order.
    use_hook(|| ());
    use_future(move || async move {});
}

/// The carousel: one `HeroCard` per resume point plus a dot row when there is
/// more than one page.
#[component]
pub(super) fn ContinueHero(points: Vec<ResumePoint>, server_url: String) -> Element {
    let page = use_signal(|| 0usize);
    use_hero_dot_sync(page);
    let count = points.len();
    rsx! {
        section {
            class: "ch-hero",
            "data-testid": "continue-hero",
            aria_label: "Continue reading",
            div { class: "ch-track", "data-testid": "continue-hero-track",
                for point in points.into_iter() {
                    HeroCard {
                        key: "{point.record.book_uuid}·{point.record.format:?}",
                        point,
                        server_url: server_url.clone(),
                    }
                }
            }
            if count > 1 {
                div { class: "ch-dots",
                    for i in 0..count {
                        button {
                            key: "{i}",
                            r#type: "button",
                            class: if i == page() { "ch-dot ch-dot--active" } else { "ch-dot" },
                            "data-testid": "hero-dot-{i}",
                            aria_label: "Show in-progress book {i + 1} of {count}",
                            onclick: {
                                let mut page = page;
                                move |_| {
                                    page.set(i);
                                    scroll_track_to(i);
                                }
                            },
                        }
                    }
                }
            }
        }
    }
}

/// One hero page: cover, eyebrow, title/author, progress meta, and the
/// format-appropriate resume CTA. Card body links to the detail page; the CTA
/// deep-links into the reader/player (same split as the user-menu row).
#[component]
fn HeroCard(point: ResumePoint, server_url: String) -> Element {
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
    // Linked cards carry the position on the primary CTA ("Play · 16h 05m")
    // per the design; unlinked keep the bare verb.
    let cta_label = {
        let verb = if is_audio { "Play" } else { "Read" };
        let position = if point.linked {
            if is_audio {
                point
                    .record
                    .audio_position_seconds
                    .map(crate::components::alignment_modal::fmt_hm)
            } else {
                point.record.progress_percent.map(|p| format!("{p}%"))
            }
        } else {
            None
        };
        match position {
            Some(p) => format!("{verb} \u{00b7} {p}"),
            None => verb.to_string(),
        }
    };
    // Dual-format but unlinked: the hero carries the link invitation the
    // design draws under the two-card state.
    let dual_unlinked = !point.linked
        && book.formats.iter().any(|f| f.eq_ignore_ascii_case("epub"))
        && book.formats.iter().any(|f| {
            matches!(
                f.to_ascii_lowercase().as_str(),
                "m4b" | "m4a" | "mp3" | "aac" | "flac" | "ogg" | "opus" | "wav"
            )
        });
    // Linked books carry the mapped "resume in the other format" candidate;
    // the CTA routes to that surface, whose own prompt offers the precise
    // jump once loaded.
    let counterpart = point.cross_format.as_ref().map(|cf| {
        let (route, label) = match cf.target {
            ProgressFormat::Audio => (
                Route::BookListen {
                    uuid: uuid.clone(),
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
                Route::BookRead { uuid: uuid.clone() },
                format!("Read \u{00b7} \u{2248} {}%", cf.percent.unwrap_or(0)),
            ),
        };
        (route, label)
    });
    // Shared dispatch (routes::resume_route) rather than a local format
    // match: it carries the audio point's `book_file_id` and sends a
    // CBZ-only book to the comic pager — the mobile resume card already
    // routes through it.
    let resume_route = crate::routes::resume_route(&point);
    let (meta, pct) = resume_meta(&point);
    let cover_bust =
        crate::contexts::cover_bust_for(crate::contexts::use_cover_cache_bust().0, &uuid);
    let (thumb_src, thumb_srcset) =
        crate::components::cover_tile::thumb_srcs(&book, &uuid, &server_url, cover_bust);

    rsx! {
        article { class: "ch-card", "data-testid": "hero-card-{uuid}",
            Link {
                to: Route::BookDetail { uuid: uuid.clone() },
                class: "ch-cover",
                aria_label: "Open details for {title}",
                crate::components::atrium::Cover {
                    book,
                    src_override: thumb_src,
                    srcset: thumb_srcset,
                    sizes: Some("96px".to_string()),
                }
            }
            div { class: "ch-body",
                span { class: "ch-eyebrow",
                    "{eyebrow}"
                    if point.linked {
                        span { class: "ch-eyebrow-glyph",
                            crate::components::sync_glyph::SyncGlyph { size: 11 }
                        }
                    }
                }
                Link {
                    to: Route::BookDetail { uuid: uuid.clone() },
                    class: "ch-title-link",
                    h3 { class: "ch-title", "{title}" }
                }
                if !author.is_empty() {
                    span { class: "ch-author", "{author}" }
                }
                if let Some(pct) = pct {
                    span { class: "ch-bar", i { style: "width:{pct}%" } }
                }
                if dual_unlinked {
                    Link {
                        to: Route::BookDetail { uuid: uuid.clone() },
                        class: "ch-link-invite",
                        "data-testid": "hero-link-invite-{uuid}",
                        crate::components::sync_glyph::SyncGlyph { size: 12 }
                        span { "Same book, two spots — link the formats to carry one position." }
                    }
                }
                div { class: "ch-foot",
                    span { class: "mono ch-meta",
                        if point.linked { "newest spot: " }
                        if point.linked {
                            if is_audio { "audiobook \u{00b7} " } else { "ebook \u{00b7} " }
                        }
                        "{meta}"
                    }
                    if let Some((route, label)) = counterpart {
                        Link {
                            to: route,
                            class: "ch-cta ch-cta-alt",
                            "data-testid": "hero-crossformat-{uuid}",
                            span { "{label}" }
                        }
                    }
                    Link {
                        to: resume_route,
                        class: "ch-cta",
                        "data-testid": "hero-resume-{uuid}",
                        aria_label: "{eyebrow}: {title}",
                        if is_audio {
                            {play_glyph(11)}
                        } else {
                            {book_glyph(11)}
                        }
                        span { "{cta_label}" }
                    }
                }
            }
        }
    }
}
