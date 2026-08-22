use super::*;

/// SSR-render the CTA row for a book with the given format availability.
fn render_cta_row(has_ebook: bool, has_audio: bool) -> String {
    render_cta_row_with_comic(has_ebook, has_audio, false)
}

fn render_cta_row_with_comic(has_ebook: bool, has_audio: bool, has_comic: bool) -> String {
    dioxus::ssr::render_element(rsx! {
        BdCtaRow {
            has_ebook,
            has_audio,
            has_comic,
            meta: BookActionMeta {
                uuid: "book-uuid".to_string(),
                ..Default::default()
            },
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
    // No reading CTAs and no export menu for a book with no files.
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
        BdCtaRow {
            has_ebook: false,
            has_audio: false,
            has_comic: true,
            meta: BookActionMeta {
                uuid: "book-uuid".to_string(),
                ..Default::default()
            },
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
        BdCtaRow {
            has_ebook: true,
            has_audio: false,
            has_comic: true,
            meta: BookActionMeta {
                uuid: "book-uuid".to_string(),
                ..Default::default()
            },
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

#[test]
fn phys_badge_renders_its_testid_and_label() {
    let html = dioxus::ssr::render_element(rsx! { BdPhysBadge {} });
    assert!(html.contains("data-testid=\"format-badge-physical\""));
    assert!(html.contains("Physical"));
}

#[test]
fn wishlist_badge_renders_its_testid_and_label() {
    let html = dioxus::ssr::render_element(rsx! { BdWishlistBadge {} });
    assert!(html.contains("data-testid=\"format-badge-wishlist\""));
    assert!(html.contains("Physical Wishlist"));
}
