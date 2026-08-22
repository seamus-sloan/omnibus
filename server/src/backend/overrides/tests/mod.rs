//! Tests for the metadata override REST endpoints — the mobile client's entry
//! points; RPC variants are covered separately in `db::queries`. Split by
//! sub-topic into the sibling modules below: override save/delete, cover
//! upload/delete, applying a provider cover by URL, and the admin
//! rewrite-all-epubs route.

mod cover;
mod cover_from_url;
mod rewrite;
mod save_delete;
