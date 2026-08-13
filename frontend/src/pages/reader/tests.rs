//! Tests for `derive_reader_display`. `Signal::new` needs a Dioxus runtime,
//! so this runs inside a `VirtualDom` (the pattern in
//! `frontend/src/pages/reader/prefs/tests.rs`).

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
