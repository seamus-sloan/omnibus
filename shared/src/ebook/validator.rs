//! Content validator for a library file, derived from the filesystem stat
//! the indexer already records. Produced by the db projection, carried on
//! [`super::BookFileInfo`], and compared by offline clients to learn that a
//! downloaded copy is stale.

/// Entity tag for a library file, derived from the `(mtime_epoch,
/// size_bytes)` pair recorded on `book_files` / `book_file_parts`.
///
/// `None` for the indexer's `(0, 0)` never-observed sentinel and for
/// negative inputs, so a client never reads the one-time stat backfill as a
/// content change. The returned string includes the surrounding quotes, so
/// it is a valid `entity-tag` if a caller sends it back as a header.
///
/// This is deliberately the *same* pair the reindex diff keys on, so "the
/// validator moved" and "the indexer classified this Changed" are one
/// statement rather than two that can drift. A stored column would be a
/// second copy of the fact that every write path must remember to bump; a
/// content hash would mean re-reading every audiobook on every scan.
///
/// Second granularity is the right precision *here* because this value is
/// only ever compared against itself across a reindex — it cannot be
/// stronger than the diff key it mirrors. The stronger, request-time
/// validator the media endpoints put on the wire is a separate thing with a
/// separate job (`If-Range`, where a miss corrupts a file rather than
/// merely missing a notification).
pub fn file_etag(size_bytes: i64, mtime_epoch: i64) -> Option<String> {
    if size_bytes < 0 || mtime_epoch < 0 {
        return None;
    }
    if size_bytes == 0 && mtime_epoch == 0 {
        return None;
    }
    Some(format!("\"{mtime_epoch:x}-{size_bytes:x}\""))
}

/// Which of a book's two downloadable formats a validator query is about.
/// The wire form is the same token the client's download registry uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadFormat {
    Epub,
    Audio,
}

impl DownloadFormat {
    /// `book_files.format` values this download format is served from, in no
    /// particular order — the lowest-ordinal matching row is the one the
    /// server resolves when no `file_id` is given.
    pub fn file_formats(self) -> &'static [&'static str] {
        match self {
            DownloadFormat::Epub => &["EPUB"],
            DownloadFormat::Audio => &["M4B", "M4A", "MP3"],
        }
    }
}

/// One "what is this file's validator now?" question.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DownloadValidatorQuery {
    pub book_uuid: String,
    pub format: DownloadFormat,
    /// The explicitly chosen `book_files` row, when the download was started
    /// against one. `None` means whichever row the server resolves by
    /// default, so a book with two editions is answered about the same file
    /// the download actually came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<i64>,
}

/// The answer to one query. `etag` is `None` when the book, the format, or
/// the chosen file is gone, or when the scanner has not stat'd it — all of
/// which a client reads as "can't tell", never as "unchanged".
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DownloadValidator {
    pub book_uuid: String,
    pub format: DownloadFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

/// `POST /api/downloads/validators` request body.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DownloadValidatorRequest {
    pub files: Vec<DownloadValidatorQuery>,
}

/// `POST /api/downloads/validators` response body.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DownloadValidatorResponse {
    pub files: Vec<DownloadValidator>,
}

/// Most files one validator request may ask about.
///
/// A device asks about what it has downloaded, which is bounded by its own
/// disk; this only exists so a malformed or hostile body can't turn one
/// request into unbounded work.
pub const MAX_VALIDATOR_QUERY: usize = 500;

#[cfg(test)]
mod tests {
    use super::file_etag;

    #[test]
    fn file_etag_encodes_mtime_and_size_as_hex() {
        assert_eq!(file_etag(4096, 255).as_deref(), Some("\"ff-1000\""));
    }

    #[test]
    fn file_etag_returns_none_for_the_never_observed_sentinel() {
        assert_eq!(file_etag(0, 0), None);
    }

    #[test]
    fn file_etag_returns_none_for_negative_inputs() {
        assert_eq!(file_etag(-1, 100), None);
        assert_eq!(file_etag(100, -1), None);
    }

    #[test]
    fn file_etag_distinguishes_a_size_change_from_an_mtime_change() {
        let base = file_etag(100, 100);
        assert_ne!(base, file_etag(101, 100));
        assert_ne!(base, file_etag(100, 101));
    }

    #[test]
    fn file_etag_covers_a_real_zero_byte_file() {
        // Only the (0, 0) pair is the sentinel. A genuinely empty file that
        // has been stat'd still has an mtime, so it gets a validator.
        assert_eq!(file_etag(0, 1).as_deref(), Some("\"1-0\""));
    }
}
