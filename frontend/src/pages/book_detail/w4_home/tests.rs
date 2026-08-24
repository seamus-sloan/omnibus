use omnibus_shared::EbookMetadata;

use super::*;

fn facts(has_ebook: bool, has_audio: bool, has_comic: bool) -> W4ViewFacts {
    W4ViewFacts {
        title: "Book".into(),
        primary_author: "Author".into(),
        author_id: None,
        authors_line: "Author".into(),
        series: None,
        has_ebook,
        has_audio,
        has_comic,
    }
}

fn book() -> EbookMetadata {
    EbookMetadata {
        unique_identifier: Some("book-uuid".into()),
        ..Default::default()
    }
}

fn no_progress() -> W4Progress {
    W4Progress {
        reading: None,
        listening: None,
    }
}

/// SSR-render the CTA row for a book with the given format availability.
fn render_cta_row(has_ebook: bool, has_audio: bool) -> String {
    dioxus::ssr::render_element(rsx! {
        W4CtaRow {
            b: book(),
            view: facts(has_ebook, has_audio, false),
            progress: no_progress(),
        }
    })
}

#[test]
fn immersive_cta_renders_when_book_has_both_ebook_and_audio() {
    let html = render_cta_row(true, true);
    assert!(html.contains("data-testid=\"immersive-read\""));
    assert!(html.contains("Immersive Read"));
}

#[test]
fn immersive_cta_absent_when_book_has_ebook_only() {
    let html = render_cta_row(true, false);
    assert!(!html.contains("data-testid=\"immersive-read\""));
}

#[test]
fn immersive_cta_absent_when_book_has_audio_only() {
    let html = render_cta_row(false, true);
    assert!(!html.contains("data-testid=\"immersive-read\""));
}

#[test]
fn fileless_book_shows_no_files_disclaimer_and_hides_reading_ctas() {
    let html = render_cta_row(false, false);
    assert!(html.contains("data-testid=\"no-files-disclaimer\""));
    assert!(!html.contains("data-testid=\"start-reading\""));
    assert!(!html.contains("data-testid=\"start-listening\""));
}

#[test]
fn book_with_files_shows_no_disclaimer() {
    let html = render_cta_row(true, false);
    assert!(!html.contains("data-testid=\"no-files-disclaimer\""));
}

// The comic CTA (and the single-file picker) render `dioxus_router::Link`,
// which panics without a parent router, so these two variants mount
// behind one-route test routers — the highlights-card pattern.
#[derive(Clone, Debug, PartialEq, dioxus_router::Routable)]
enum ComicOnlyRoute {
    #[route("/")]
    ComicOnlyHost {},
}

#[component]
fn ComicOnlyHost() -> Element {
    rsx! {
        W4CtaRow {
            b: book(),
            view: facts(false, false, true),
            progress: no_progress(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, dioxus_router::Routable)]
enum EpubAndComicRoute {
    #[route("/")]
    EpubAndComicHost {},
}

#[component]
fn EpubAndComicHost() -> Element {
    rsx! {
        W4CtaRow {
            b: book(),
            view: facts(true, false, true),
            progress: no_progress(),
        }
    }
}

#[test]
fn comic_only_book_shows_the_pager_cta_instead_of_the_disclaimer() {
    let html = crate::test_support::render_in_vdom(|| {
        rsx! {
            dioxus_router::Router::<ComicOnlyRoute> {}
        }
    });
    assert!(
        html.contains("data-testid=\"start-reading-comic\""),
        "{html}"
    );
    assert!(html.contains("/comic/book-uuid"), "{html}");
    assert!(
        !html.contains("data-testid=\"no-files-disclaimer\""),
        "{html}"
    );
}

#[test]
fn epub_primary_wins_over_the_comic_cta_when_both_formats_exist() {
    let html = crate::test_support::render_in_vdom(|| {
        rsx! {
            dioxus_router::Router::<EpubAndComicRoute> {}
        }
    });
    assert!(html.contains("data-testid=\"start-reading\""), "{html}");
    assert!(
        !html.contains("data-testid=\"start-reading-comic\""),
        "{html}"
    );
}

// The single-file picker renders a `Link`, so the resume-verb variant mounts
// behind a one-route test router like the comic hosts above.
#[derive(Clone, Debug, PartialEq, dioxus_router::Routable)]
enum ResumeRoute {
    #[route("/")]
    ResumeHost {},
}

#[component]
fn ResumeHost() -> Element {
    use omnibus_shared::progress::{ProgressFormat, ProgressRecord};
    let progress = W4Progress {
        reading: Some(ProgressRecord {
            book_uuid: "book-uuid".into(),
            format: ProgressFormat::Epub,
            epub_cfi: None,
            audio_position_seconds: None,
            progress_percent: Some(40),
            kobo_location: None,
            book_file_id: None,
            updated_at: 0,
            client_updated_at: 0,
        }),
        listening: None,
    };
    rsx! {
        W4CtaRow {
            b: book(),
            view: facts(true, false, false),
            progress,
        }
    }
}

#[test]
fn resume_verbs_take_over_once_a_position_exists() {
    let html = crate::test_support::render_in_vdom(|| {
        rsx! {
            dioxus_router::Router::<ResumeRoute> {}
        }
    });
    assert!(html.contains("Resume reading"), "{html}");
}
