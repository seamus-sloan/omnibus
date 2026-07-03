//! Hardcover community-list co-occurrence suggestions ("Readers also enjoyed").
//! [`hardcover`] is the GraphQL client, [`filter`] the pure ranking/trim logic,
//! [`data`] the cache CRUD + [`data::decide`] de-dup state machine, and
//! [`cascade`] the resolution orchestrator driven by
//! [`crate::worker::Task::ResolveSuggestions`].

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
