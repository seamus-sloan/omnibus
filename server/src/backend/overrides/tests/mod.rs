//! Tests for metadata override REST endpoints (the mobile client's entry
//! points; RPC variants are covered separately in `db::queries`).
//!
//! Split by sub-topic (mirrors `opds/tests/`): override save/delete,
//! cover upload/delete, and the admin rewrite-all-epubs route each live
//! in their own sibling module below.

mod cover;
mod rewrite;
mod save_delete;
