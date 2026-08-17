//! Tests for metadata override handlers.
// -------------------------------------------------------------------
// F5.1 metadata-override REST endpoints (issue #105).
//
// The RPC variants (`rpc_save_overrides`, `rpc_delete_overrides`) are
// covered by DB-level unit tests in `db::queries`; these integration
// tests cover the REST entry points the mobile client uses.
//
// Split by sub-topic (mirrors `server/src/backend/opds/tests/`): override
// save/delete, cover upload/delete, and the admin rewrite-all-epubs route
// each live in their own sibling module below.
// -------------------------------------------------------------------

mod cover;
mod rewrite;
mod save_delete;
