//! Tests for `derive_reader_display` (needs a Dioxus runtime for
//! `Signal::new`, the pattern in `frontend/src/pages/reader/prefs/tests.rs`)
//! and an SSR render-smoke check on `ReaderViewerStage`'s error overlay.

use super::*;

#[test]
fn derive_reader_display_blanks_chapter_title_while_loading_and_restores_it_once_ready() {
    #[component]
    fn AssertDisplay() -> Element {
        let loc = Signal::new(RelocateData {
            chapter: 5,
            total_chapters: 94,
            chapter_title: "Chapter Five".to_string(),
            pct: 12,
            ..Default::default()
        });
        let book_meta: Signal<Option<omnibus_shared::EbookMetadata>> = Signal::new(None);

        let loading = derive_reader_display(loc, book_meta, ReaderStatus::Loading);
        assert_eq!(loading.chapter_title, "");
        assert_eq!(loading.title_sub, "");

        let ready = derive_reader_display(loc, book_meta, ReaderStatus::Ready);
        assert_eq!(ready.chapter_title, "Chapter Five");
        assert!(!ready.title_sub.is_empty());

        rsx! {}
    }
    VirtualDom::new(AssertDisplay).rebuild_in_place();
}

// SSR render-smoke coverage of the error overlay — separate module because
// this needs the `server` feature (`dioxus::ssr`), while the pure
// `derive_reader_display` test above runs under any target.
#[cfg(all(test, feature = "server"))]
mod render_tests {
    use super::*;
    use crate::test_support::render;

    #[component]
    fn ViewerStageHarness(status: ReaderStatus) -> Element {
        rsx! {
            ReaderViewerStage {
                status,
                on_retry: EventHandler::new(|_| {}),
            }
        }
    }

    #[test]
    fn reader_viewer_stage_renders_a_retry_action_when_failed() {
        let html = render(rsx! { ViewerStageHarness { status: ReaderStatus::Failed } });
        assert!(html.contains("data-testid=\"reader-error\""));
        assert!(html.contains("data-testid=\"reader-retry\""));
        assert!(html.contains("Retry"));
    }

    #[test]
    fn reader_viewer_stage_omits_the_error_overlay_when_ready() {
        let html = render(rsx! { ViewerStageHarness { status: ReaderStatus::Ready } });
        assert!(!html.contains("reader-error"));
        assert!(!html.contains("reader-retry"));
    }
}
