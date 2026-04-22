//! Omnibus data layer: schema migrations, SQLite pool init, the normalized
//! query layer, and the indexing pipeline (file scan → EPUB metadata
//! extraction → atomic per-library upsert). Consumed by both `server/`
//! (axum REST handlers) and `frontend/` (Dioxus server functions under
//! `feature = "server"`).

pub mod ebook;
pub mod indexer;
pub mod queries;
pub mod scanner;

// Flatten the query layer so callers write `omnibus_db::list_books(...)`
// instead of `omnibus_db::queries::list_books(...)`. Keeps callsites terse
// and mirrors how `db.rs` looked before the extraction.
pub use queries::*;
