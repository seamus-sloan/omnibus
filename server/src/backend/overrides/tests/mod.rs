//! Tests for the metadata override REST endpoints — the mobile client's entry
//! points; RPC variants are covered separately in `db::queries`. Split by
//! sub-topic into the sibling modules below: override save/delete, cover
//! upload/delete, and the admin rewrite-all-epubs route.

mod cover;
mod rewrite;
mod save_delete;
