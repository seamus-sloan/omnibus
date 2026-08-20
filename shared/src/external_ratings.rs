//! Community ratings published by the metadata providers — "what the internet
//! gave it", as opposed to the per-user star rating in [`crate::ratings`],
//! which is "what I gave it". A book carries several at once, one per source,
//! and they are provider-authored and refreshable rather than user-authored.

use serde::{Deserialize, Serialize};

use crate::metadata_lookup::MetadataProvider;

/// What one provider says about a book's community rating.
///
/// The score is kept on the provider's **own** scale, with that scale
/// alongside it. Every source today is out of 5, so normalizing on the way in
/// looks free — but a future 0–10 source would then need a backfill *and* a
/// way to tell already-normalized values from raw ones. One extra field
/// removes that problem permanently.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderRating {
    pub rating: f64,
    /// Top of the provider's scale (`5.0` for all three of today's sources).
    pub rating_max: f64,
    /// How many people rated it, when the provider says so. `None` means it
    /// didn't say — which is not the same as nobody having rated it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratings_count: Option<i64>,
    /// The provider's own page for this book, for the attribution link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

impl ProviderRating {
    /// Maximum **byte** length of a stored `source_url`, matching
    /// [`crate::AUTHOR_PHOTO_URL_MAX_LEN`] — the other provider-supplied URL
    /// this codebase stores.
    pub const SOURCE_URL_MAX_LEN: usize = crate::AUTHOR_PHOTO_URL_MAX_LEN;

    /// Build a rating from a provider's raw pair, or `None` when the provider
    /// has nothing to report for this book.
    ///
    /// **Absent is not zero.** Every provider signals "nobody has rated this"
    /// by omitting the field or answering `0`, and a stored `0` would render
    /// as a genuine "0/5" verdict the source never gave. A count of `0` is
    /// dropped to `None` for the same reason, and an over-long `source_url` is
    /// dropped rather than truncated — a mangled link is worse than none.
    pub fn new(
        rating: Option<f64>,
        rating_max: f64,
        ratings_count: Option<i64>,
        source_url: Option<String>,
    ) -> Option<Self> {
        let rating = rating.filter(|r| r.is_finite() && *r > 0.0 && *r <= rating_max)?;
        if !rating_max.is_finite() || rating_max <= 0.0 {
            return None;
        }
        Some(Self {
            rating,
            rating_max,
            ratings_count: ratings_count.filter(|c| *c > 0),
            source_url: source_url.filter(|u| !u.is_empty() && u.len() <= Self::SOURCE_URL_MAX_LEN),
        })
    }
}

/// One stored community rating, attributed to the source that published it.
///
/// Returned on the book-detail payload and rendered *beside* the reader's own
/// star rating, never merged into it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalRating {
    pub provider: MetadataProvider,
    pub display_name: String,
    pub rating: f64,
    pub rating_max: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratings_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// Unix seconds at which this row was last refreshed from the provider.
    pub fetched_at: i64,
}

impl ExternalRating {
    /// Attribute a freshly-fetched [`ProviderRating`] to its source.
    pub fn new(provider: MetadataProvider, rating: ProviderRating, fetched_at: i64) -> Self {
        Self {
            provider,
            display_name: provider.display_name().to_string(),
            rating: rating.rating,
            rating_max: rating.rating_max,
            ratings_count: rating.ratings_count,
            source_url: rating.source_url,
            fetched_at,
        }
    }
}

#[cfg(test)]
mod tests;
