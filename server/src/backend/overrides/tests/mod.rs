//! Tests for metadata override REST endpoints (the mobile client's entry
//! points; RPC variants are covered separately in `db::queries`).
//!
//! Split by sub-topic (mirrors `opds/tests/`): override save/delete,
//! cover upload/delete, applying a provider cover by URL, and the admin
//! rewrite-all-epubs route each live in their own sibling module below.

mod cover;
mod cover_from_url;
mod rewrite;
mod save_delete;
