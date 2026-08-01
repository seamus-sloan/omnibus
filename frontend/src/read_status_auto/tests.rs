use omnibus_shared::ReadStatus;

use super::auto_transition;

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
