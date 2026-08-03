//! Transition-table tests for [`auto_transition`], plus the effectful
//! hook's pulled-out decision points: [`apply_fetched_status`]'s `load_seq`
//! guard and [`decide_transition_write`]'s write decision (including the
//! never-downgrade case). Driven the same way `pages/comic_reader.rs` drives
//! `apply_bootstrap_outcome` — signals constructed inside a mounted dummy
//! component (`VirtualDom::rebuild_in_place`), since a `Signal` needs an
//! active Dioxus scope to read or write.

use dioxus::prelude::*;
use omnibus_shared::{ReadStatus, ReadStatusRecord, SetReadStatus};

use super::{apply_fetched_status, auto_transition, decide_transition_write};

#[test]
fn auto_transition_marks_an_unread_book_reading_on_open() {
    assert_eq!(
        auto_transition(ReadStatus::Unread, false),
        Some(ReadStatus::Reading)
    );
}

#[test]
fn auto_transition_leaves_reading_and_finished_untouched_on_open() {
    assert_eq!(auto_transition(ReadStatus::Reading, false), None);
    assert_eq!(auto_transition(ReadStatus::Finished, false), None);
}

#[test]
fn auto_transition_marks_any_unfinished_book_finished_at_the_end() {
    assert_eq!(
        auto_transition(ReadStatus::Unread, true),
        Some(ReadStatus::Finished)
    );
    assert_eq!(
        auto_transition(ReadStatus::Reading, true),
        Some(ReadStatus::Finished)
    );
}

#[test]
fn auto_transition_never_rewrites_an_already_finished_book() {
    assert_eq!(auto_transition(ReadStatus::Finished, true), None);
}

fn stub_record(status: ReadStatus) -> ReadStatusRecord {
    ReadStatusRecord {
        book_uuid: "ignored".to_string(),
        status,
        updated_at: 0,
        finished_at: None,
    }
}

/// A fetch that lands under the current `load_seq` populates `status`, and
/// the write effect's decision function turns that into the transitioned
/// write — the happy path the hook drives on every book open.
#[test]
fn apply_fetched_status_lands_and_decide_transition_write_persists_the_transition() {
    #[component]
    fn AssertFetchLandsAndWrites() -> Element {
        let load_seq: Signal<u64> = Signal::new(1);
        let status: Signal<Option<(String, ReadStatus)>> = Signal::new(None);

        apply_fetched_status(
            Some(stub_record(ReadStatus::Unread)),
            1,
            load_seq,
            "book-a".to_string(),
            status,
        );
        assert_eq!(
            *status.read(),
            Some(("book-a".to_string(), ReadStatus::Unread)),
            "a fetch for the current load must land"
        );

        let update = decide_transition_write(status, false);
        assert_eq!(
            update,
            Some(SetReadStatus {
                book_uuid: "book-a".to_string(),
                status: ReadStatus::Reading,
            }),
            "opening an unread book must write Reading"
        );
        assert_eq!(
            *status.read(),
            Some(("book-a".to_string(), ReadStatus::Reading)),
            "the optimistic value lands before the write resolves"
        );

        rsx! {}
    }
    VirtualDom::new(AssertFetchLandsAndWrites).rebuild_in_place();
}

/// A fetch answering a navigation `load_seq` has already superseded must not
/// land — the guard `ComicReadPage`'s `apply_bootstrap_outcome` shares the
/// shape of.
#[test]
fn apply_fetched_status_drops_a_response_superseded_by_a_newer_navigation() {
    #[component]
    fn AssertStaleFetchDropped() -> Element {
        let load_seq: Signal<u64> = Signal::new(2);
        let status: Signal<Option<(String, ReadStatus)>> = Signal::new(None);

        // Load 1's response lands after load 2 has already started.
        apply_fetched_status(
            Some(stub_record(ReadStatus::Reading)),
            1,
            load_seq,
            "stale-book".to_string(),
            status,
        );
        assert!(
            status.read().is_none(),
            "a fetch superseded by a newer navigation must not land"
        );

        // Load 2's own response is current and must land.
        apply_fetched_status(
            Some(stub_record(ReadStatus::Reading)),
            2,
            load_seq,
            "current-book".to_string(),
            status,
        );
        assert_eq!(
            *status.read(),
            Some(("current-book".to_string(), ReadStatus::Reading))
        );

        rsx! {}
    }
    VirtualDom::new(AssertStaleFetchDropped).rebuild_in_place();
}

/// Opening a book already `Finished` (`at_end: false`, the "merely open"
/// case) must never write `Reading` over it.
#[test]
fn decide_transition_write_never_downgrades_a_finished_book_on_open() {
    #[component]
    fn AssertNoDowngradeOnOpen() -> Element {
        let status: Signal<Option<(String, ReadStatus)>> =
            Signal::new(Some(("finished-book".to_string(), ReadStatus::Finished)));

        let update = decide_transition_write(status, false);

        assert_eq!(
            update, None,
            "opening a finished book must not queue a downgrade to Reading"
        );
        assert_eq!(
            *status.read(),
            Some(("finished-book".to_string(), ReadStatus::Finished)),
            "status must stay Finished when nothing was written"
        );

        rsx! {}
    }
    VirtualDom::new(AssertNoDowngradeOnOpen).rebuild_in_place();
}
