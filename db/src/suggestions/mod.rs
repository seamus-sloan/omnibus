//! F3.3 "Readers also enjoyed" — Hardcover community-list co-occurrence.
//!
//! [`hardcover`] talks to the Hardcover GraphQL API; [`filter`] holds the pure
//! different-author / different-series / entry-point trimming; [`data`] is the
//! cache CRUD + the [`data::decide`] de-duplication state machine; [`cascade`]
//! orchestrates resolution and is driven by
//! [`crate::worker::Task::ResolveSuggestions`].
//!
//! Caching guarantees a page refresh or a burst of concurrent viewers never
//! re-hits Hardcover while a result is fresh: a 30-day TTL result cache, a
//! `pending` debounce so concurrent first-viewers collapse to one posted task,
//! and worker resource-key serialization with a cascade re-check on top.

pub mod cascade;
pub mod data;
pub mod filter;
pub mod hardcover;

pub use cascade::{resolve, resolve_with};
pub use data::{
    decide, delete_suggestions, get_suggestion_cover, get_suggestions, mark_pending,
    replace_suggestions, suggestion_state, CacheDecision, CachedSuggestion, NewSuggestion,
    SuggestionState, SuggestionsDataError, PENDING_DEBOUNCE_SECS, SUGGESTIONS_TTL_SECS,
};
pub use filter::{filter_candidates, is_entry_point, is_same_author, is_same_series, Candidate};
pub use hardcover::{HardcoverConfig, HardcoverError};

#[cfg(test)]
mod tests;
