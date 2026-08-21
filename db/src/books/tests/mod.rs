//! Unit tests for the `books` module, split by sub-topic into the sibling
//! modules below — mirroring the production split (`get`, `list`, `search`,
//! …). The cross-cutting seed helpers they share live in
//! `crate::test_support`, so each module imports what it needs from there.

mod files;
mod get;
mod identity;
mod isbn;
mod list;
mod overrides;
mod search;
mod validators;
