//! Per-user star-rating wire types shared between web and mobile clients.
//!
//! A rating is a half-star value in `0.5..=5.0`, validated at the handler
//! boundary and stored as an exact half-star count. Half values are exactly
//! representable in `f32`, so equality comparisons are reliable.

use serde::{Deserialize, Serialize};

/// Inclusive bounds for a star rating (half-star steps).
pub const MIN_STARS: f32 = 0.5;
pub const MAX_STARS: f32 = 5.0;

/// Convert a stored half-star count (`1..=10`) to a `0.5..=5.0` star value.
pub fn stars_from_half_stars(half_stars: i64) -> f32 {
    // Stored counts are 1..=10 by construction; the clamp documents that
    // bound and keeps the u16 → f32 conversion lossless.
    f32::from(u16::try_from(half_stars.clamp(0, 10)).unwrap_or(0)) / 2.0
}

/// Write payload: set (or change) the current user's rating for a book.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RatingUpdate {
    pub book_uuid: String,
    /// Star rating in `0.5..=5.0`, in half-star steps.
    pub stars: f32,
}

impl RatingUpdate {
    /// Reject an empty uuid, a non-finite/out-of-range value, or a value that
    /// isn't a whole number of half-stars. Handlers translate `Err(_)` into 400.
    pub fn validate(&self) -> Result<(), String> {
        if self.book_uuid.trim().is_empty() {
            return Err("book_uuid is required".into());
        }
        if !self.stars.is_finite() || self.stars < MIN_STARS || self.stars > MAX_STARS {
            return Err(format!("stars must be between {MIN_STARS} and {MAX_STARS}"));
        }
        if (self.stars * 2.0).fract() != 0.0 {
            return Err("stars must be in half-star (0.5) steps".into());
        }
        Ok(())
    }

    /// Storage form: the rating expressed as a count of half-stars (`1..=10`).
    pub fn half_stars(&self) -> i64 {
        // `validate` bounds stars to 0.5..=5.0; the clamp repeats the bound so
        // an unvalidated value still lands in-range (NaN casts to 0).
        let scaled = (self.stars * 2.0).round().clamp(0.0, 10.0);
        #[allow(clippy::cast_possible_truncation)]
        let half = scaled as i64;
        half
    }
}

/// Server-authoritative rating returned by the set/get endpoints. `updated_at`
/// is unix seconds (SQLite `strftime('%s')`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RatingRecord {
    pub book_uuid: String,
    /// Star rating in `0.5..=5.0`, in half-star steps.
    pub stars: f32,
    pub updated_at: i64,
}

/// A rating with the user identity needed for an attributed book-detail row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttributedRating {
    pub user_id: i64,
    pub username: String,
    /// Star rating in `0.5..=5.0`, in half-star steps.
    pub stars: f32,
    pub updated_at: i64,
}

#[cfg(test)]
mod tests;
