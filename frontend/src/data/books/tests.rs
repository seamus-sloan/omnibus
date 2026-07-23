//! `get_ebooks_page` policy tests: known-offline replica fast path, the
//! cached-first-page (SWR) short-circuit, and the replica fallback when the
//! first fetch dies mid-request.

// The state-lock guard is deliberately held across awaits: it serializes
// whole async test bodies against process-global state, and each test owns
// its own thread + runtime, so there is no interleaving to deadlock on.
#![allow(clippy::await_holding_lock)]

use omnibus_shared::Contributor;

use crate::offline::cache;
use crate::offline::store;
use crate::offline::sync::test_state_lock;

use super::*;

fn book(title: &str) -> EbookMetadata {
    EbookMetadata {
        title: Some(title.to_string()),
        filename: format!("{title}.epub"),
        creators: vec![Contributor {
            name: "Author".into(),
            ..Default::default()
        }],
        formats: vec!["EPUB".into()],
        unique_identifier: Some(format!("uuid-{title}")),
        ..Default::default()
    }
}

fn titles(page: &LibraryPage) -> Vec<String> {
    page.books
        .iter()
        .map(|b| b.title.clone().unwrap_or_default())
        .collect()
}

#[tokio::test]
async fn get_ebooks_page_serves_replica_when_known_offline() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    cache::put_json(
        &cache::keys::ebooks_all(),
        &vec![book("Dune"), book("Atonement")],
    );
    crate::offline::sync::note_offline();

    let page = get_ebooks_page(
        "http://127.0.0.1:1",
        SortKey::Title,
        SortDir::Asc,
        ViewFilters::default(),
        None,
        10,
    )
    .await
    .expect("replica page");
    assert_eq!(titles(&page), vec!["Atonement", "Dune"]);
    crate::offline::sync::note_online();
}

#[tokio::test]
async fn get_ebooks_page_serves_cached_first_page_without_network() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    let key = cache::keys::ebooks_first(SortKey::Title.as_wire(), SortDir::Asc.as_wire(), "");
    cache::put_json(
        &key,
        &LibraryPage {
            path: None,
            books: vec![book("Cached Book")],
            next_cursor: None,
            total: Some(1),
            facets: None,
        },
    );

    // Server unreachable, but the fresh cached first page short-circuits
    // before any network attempt.
    let page = get_ebooks_page(
        "http://127.0.0.1:1",
        SortKey::Title,
        SortDir::Asc,
        ViewFilters::default(),
        None,
        10,
    )
    .await
    .expect("cached first page");
    assert_eq!(titles(&page), vec!["Cached Book"]);
    crate::offline::sync::note_online();
}

#[tokio::test]
async fn get_ebooks_page_falls_back_to_replica_when_the_first_fetch_dies() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    // Cold first-page cache (fresh unique sort axis), warm replica.
    cache::put_json(&cache::keys::ebooks_all(), &vec![book("Beloved")]);
    store::store()
        .expect("store")
        .kv_delete(&cache::keys::ebooks_first(
            SortKey::Author.as_wire(),
            SortDir::Desc.as_wire(),
            "",
        ));

    let page = get_ebooks_page(
        "http://127.0.0.1:1",
        SortKey::Author,
        SortDir::Desc,
        ViewFilters::default(),
        None,
        10,
    )
    .await
    .expect("replica fallback");
    assert_eq!(titles(&page), vec!["Beloved"]);
    crate::offline::sync::note_online();
}
