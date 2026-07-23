//! Wire types for the admin book-deletion surface
//! (`/api/rpc/books/delete-files`).
//!
//! The dialog deletes *items*, not just files: every `book_files` row plus
//! every physical copy the book carries. That distinction is what decides
//! whether the book row survives — a book whose last remaining item is a
//! physical copy keeps its record, since a copy on a shelf isn't something a
//! delete can remove from disk.

use serde::{Deserialize, Serialize};

use crate::ebook::BookFileInfo;
use crate::physical::PhysicalCopy;

/// Everything the delete dialog needs for one book: the deletable items and
/// the user data a total delete would take with them.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookDeletionManifest {
    pub files: Vec<BookFileInfo>,
    pub copies: Vec<PhysicalCopy>,
    pub impact: BookDeletionImpact,
}

impl BookDeletionManifest {
    /// Total deletable items — the count a full selection has to reach for the
    /// book record itself to go.
    pub fn item_count(&self) -> usize {
        self.files.len() + self.copies.len()
    }
}

/// What a total delete would take with the book — the numbers the confirm
/// dialog names before an admin commits.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookDeletionImpact {
    pub highlights: i64,
    pub journal_entries: i64,
    pub bookmarks: i64,
    pub reading_sessions: i64,
    pub listening_sessions: i64,
    pub ratings: i64,
    pub shelves: i64,
}

impl BookDeletionImpact {
    /// Human-readable phrases for each non-zero count, in the order the dialog
    /// lists them (`["3 highlights", "1 rating"]`). Empty when the book
    /// carries no user data at all.
    pub fn losses(&self) -> Vec<String> {
        [
            (self.highlights, "highlight", "highlights"),
            (self.journal_entries, "journal entry", "journal entries"),
            (self.bookmarks, "bookmark", "bookmarks"),
            (self.reading_sessions, "reading session", "reading sessions"),
            (
                self.listening_sessions,
                "listening session",
                "listening sessions",
            ),
            (self.ratings, "rating", "ratings"),
            (self.shelves, "shelf placement", "shelf placements"),
        ]
        .iter()
        .filter(|(n, _, _)| *n > 0)
        .map(|(n, one, many)| format!("{n} {}", if *n == 1 { one } else { many }))
        .collect()
    }
}

/// What a delete actually removed.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteBookFilesResult {
    pub deleted_file_ids: Vec<i64>,
    pub deleted_copy_ids: Vec<i64>,
    /// True when the book row itself was removed, so the caller must navigate
    /// away instead of refetching a page that no longer exists.
    pub book_deleted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn losses_singularizes_and_skips_zero_counts() {
        let impact = BookDeletionImpact {
            highlights: 3,
            ratings: 1,
            ..Default::default()
        };

        assert_eq!(impact.losses(), vec!["3 highlights", "1 rating"]);
    }

    #[test]
    fn losses_is_empty_for_a_book_with_no_user_data() {
        assert!(BookDeletionImpact::default().losses().is_empty());
    }

    #[test]
    fn item_count_spans_files_and_physical_copies() {
        let manifest = BookDeletionManifest {
            files: vec![BookFileInfo::default(), BookFileInfo::default()],
            copies: vec![PhysicalCopy {
                id: 1,
                book_uuid: "u".into(),
                isbn: None,
                added_by_user_id: None,
                checked_in_at: 0,
                note: None,
            }],
            ..Default::default()
        };

        assert_eq!(manifest.item_count(), 3);
    }
}
