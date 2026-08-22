//! Server-authoritative reading/listening position sync plus batched session
//! reports. Position upserts are most-recent-wins on
//! `(user_id, book_uuid, format)` by client event time, not server receipt
//! time; session inserts go to the per-format `reading_sessions` /
//! `listening_sessions` tables. All rows soft-reference `books.uuid`.

use omnibus_shared::ProgressFormat;

mod resume;
mod session;
mod state;
mod upsert;

#[cfg(test)]
mod tests;

pub use resume::{recent_progress, resume_points};
pub use session::{insert_session_tx, record_session, record_session_tx};
pub use state::{
    attach_derived_kobo_location, attach_derived_percent, derive_epub_percent, get_playback_rate,
    get_progress, set_kobo_statistics_tx, set_playback_rate, spawn_epub_percent_derivation,
    KoboStatistics,
};
pub use upsert::{upsert_progress, upsert_progress_tx};

/// Failure space for the position and session-report writes.
#[derive(Debug, thiserror::Error)]
pub enum ProgressError {
    #[error("book not found")]
    BookNotFound,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<crate::hls::HlsError> for ProgressError {
    fn from(e: crate::hls::HlsError) -> Self {
        match e {
            crate::hls::HlsError::Db(inner) => ProgressError::Sqlx(inner),
        }
    }
}

impl From<crate::books::BooksError> for ProgressError {
    fn from(e: crate::books::BooksError) -> Self {
        match e {
            crate::books::BooksError::Db(inner) => ProgressError::Sqlx(inner),
            // `resolve_book_id_by_uuid` is the only `BooksError`-returning
            // call this module makes, and it never reads overrides, so neither
            // remaining variant is reachable here in practice. Fold them into a
            // generic decode error rather than panicking so a future caller
            // can't introduce an unhandled path silently.
            crate::books::BooksError::OverridesJson(inner) => {
                ProgressError::Sqlx(sqlx::Error::Decode(Box::new(inner)))
            }
            crate::books::BooksError::Other(msg) => {
                ProgressError::Sqlx(sqlx::Error::Decode(msg.into()))
            }
        }
    }
}

fn format_str(f: ProgressFormat) -> &'static str {
    match f {
        ProgressFormat::Epub => "epub",
        ProgressFormat::Audio => "audio",
    }
}

fn parse_format(raw: &str) -> ProgressFormat {
    // The CHECK constraint on `reading_progress.format` rules out anything
    // other than these two values, so an unknown string here would be a
    // schema-level invariant breach. Default to Epub rather than panic.
    match raw {
        "audio" => ProgressFormat::Audio,
        _ => ProgressFormat::Epub,
    }
}
