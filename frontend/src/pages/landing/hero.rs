//! Continue-reading hero carousel at the top of the web landing page: up to
//! [`super::effects::HERO_POINTS`] in-progress books as full-width cards in a
//! scroll-snap track with paging dots, mirroring the iOS `ContinueHero`.
//! Rendered only when at least one resume point exists, so SSR (empty signal)
//! and the first WASM paint agree — rule 07.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{ProgressFormat, ResumePoint};

use super::resume_meta::resume_meta;
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
    let (eyebrow, cta_label) = if is_audio {
        ("Continue listening", "Play")
    } else {
        ("Continue reading", "Read")
    };
    let resume_route = if is_audio {
        Route::BookListen {
            uuid: uuid.clone(),
            file_id: None,
        }
    } else {
        Route::BookRead { uuid: uuid.clone() }
    };
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
                span { class: "ch-eyebrow", "{eyebrow}" }
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
                div { class: "ch-foot",
                    span { class: "mono ch-meta", "{meta}" }
                    Link {
                        to: resume_route,
                        class: "ch-cta",
                        "data-testid": "hero-resume-{uuid}",
                        aria_label: "{eyebrow}: {title}",
                        "{cta_label}"
                    }
                }
            }
        }
    }
}
