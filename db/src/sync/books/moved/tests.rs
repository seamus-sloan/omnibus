//! Unit tests for the pure part-path rebase helper. The transactional
//! relocation writer itself is covered end-to-end from
//! `db/src/sync/books/tests.rs` and `db/src/sync/audiobooks/tests.rs`.

use super::rebase_part;

#[test]
fn rebase_part_rewrites_a_part_inside_a_moved_group_directory() {
    assert_eq!(
        rebase_part("Author/Book/01.mp3", "Author/Book", "Moved/Book"),
        Some("Moved/Book/01.mp3".to_string())
    );
}

#[test]
fn rebase_part_rewrites_a_single_file_group_whose_part_is_the_group_itself() {
    assert_eq!(
        rebase_part("Author/Book.m4b", "Author/Book.m4b", "Moved/Book.m4b"),
        Some("Moved/Book.m4b".to_string())
    );
}

#[test]
fn rebase_part_declines_a_sibling_directory_sharing_the_group_name_prefix() {
    // `Author/Book` must not swallow `Author/Bookshelf` — the separator
    // check is the whole point of doing this in Rust.
    assert_eq!(
        rebase_part("Author/Bookshelf/01.mp3", "Author/Book", "Moved/Book"),
        None
    );
}

#[test]
fn rebase_part_declines_a_part_that_does_not_sit_under_its_group() {
    assert_eq!(
        rebase_part("Elsewhere/01.mp3", "Author/Book", "Moved/Book"),
        None
    );
}
