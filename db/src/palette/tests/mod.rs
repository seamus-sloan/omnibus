//! Unit tests for the search-palette query layer, split by sub-topic into
//! the sibling modules below: per-arm matching, override-aware hits and
//! counts, library scoping, the direct arm functions, genres, the
//! `*_for_paths` variants, and physical-only visibility.

mod arms;
mod direct_arms;
mod for_paths;
mod genres;
mod overrides;
mod physical;
mod taxonomy_counts;
