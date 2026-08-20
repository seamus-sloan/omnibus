//! Cross-format position mapping: the per-user link record that turns sync
//! on for one dual-format book, the concatenated audio timeline, the linear
//! percent ↔ seconds mapping, and the resume-candidate composition the REST
//! endpoint serves. Follows `kobo_position`'s rule — every failure degrades
//! to no answer, never a wrong one; unlinked books get no answer at all.
//! Split across the sub-modules below, re-exported here as
//! `omnibus_db::cross_format::*`.

use crate::progress;

mod alignment;
mod anchors;
mod declare;
mod link;
mod mapping;
mod order;
mod resume;

#[cfg(test)]
mod tests;

pub use alignment::alignment_view;
pub use declare::declare_sync_point;
pub use link::{confirm_link, delete_link, get_link, set_follow, upsert_link, CrossFormatLink};
pub use mapping::{
    audio_timeline, derive_candidate_cfi, link_is_stale, map_audio_to_percent,
    map_percent_to_audio, AudioTimeline, TimelineFile,
};
pub use order::set_audio_order;
pub use resume::resume_candidate;

#[derive(Debug, thiserror::Error)]
pub enum CrossFormatError {
    #[error("book not found")]
    BookNotFound,
    #[error("audio files changed since the alignment loaded")]
    AudioSetMismatch,
    #[error("confirm the alignment first — this book has several audio files")]
    LinkRequired,
    #[error("the other format has no position to pair with yet")]
    CounterpartMissing,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<crate::books::BooksError> for CrossFormatError {
    fn from(e: crate::books::BooksError) -> Self {
        match e {
            crate::books::BooksError::Db(inner) => CrossFormatError::Sqlx(inner),
            // This module never decodes overrides JSON; fold the variant
            // rather than panic so a future caller can't slip through.
            crate::books::BooksError::OverridesJson(inner) => {
                CrossFormatError::Sqlx(sqlx::Error::Decode(Box::new(inner)))
            }
        }
    }
}

impl From<crate::epub_structure::EpubStructureError> for CrossFormatError {
    fn from(e: crate::epub_structure::EpubStructureError) -> Self {
        match e {
            crate::epub_structure::EpubStructureError::Sqlx(inner) => CrossFormatError::Sqlx(inner),
        }
    }
}

impl From<crate::hls::HlsError> for CrossFormatError {
    fn from(e: crate::hls::HlsError) -> Self {
        match e {
            crate::hls::HlsError::Db(inner) => CrossFormatError::Sqlx(inner),
        }
    }
}

impl From<progress::ProgressError> for CrossFormatError {
    fn from(e: progress::ProgressError) -> Self {
        match e {
            progress::ProgressError::BookNotFound => CrossFormatError::BookNotFound,
            progress::ProgressError::Sqlx(inner) => CrossFormatError::Sqlx(inner),
        }
    }
}
