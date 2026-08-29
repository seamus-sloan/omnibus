//! Read-only MCP stdio server for Omnibus. Authenticates to an instance over
//! `POST /api/auth/login` and exposes the library, search, discovery, shelf,
//! stats, progress, and annotation reads as MCP tools. Tool result schemas
//! derive from the `omnibus_shared` wire types, so the tool surface tracks
//! what the server actually serializes.
//!
//! # Write policy
//!
//! The complete set of mutating requests this crate may issue is
//! [`client::WRITE_ALLOWLIST`] — today, only the login call itself. Per the
//! offline-writes taxonomy (`.claude/rules/08-offline-writes.md`): later
//! issues may add **content-state** write tools (progress, ratings, read
//! status, annotations, journals, shelf membership); **instance
//! configuration** (settings, API keys, SMTP, metadata overrides' server
//! config) and **commands** (reindex, scan, FTS rebuild, send-to-Kindle/Kobo)
//! are never exposed as tools.
//!
//! # Configuration
//!
//! CLI flags win over environment variables:
//!
//! | Flag | Env var | Meaning |
//! |---|---|---|
//! | `--url` | `OMNIBUS_MCP_URL` | Instance base URL, e.g. `http://localhost:3000` |
//! | `--username` | `OMNIBUS_MCP_USERNAME` | Account to log in as |
//! | `--password` | `OMNIBUS_MCP_PASSWORD` | That account's password |
//!
//! The credentials are held (not just the token) because a bearer session
//! idle-expires after 7 days (`SESSION_IDLE_TIMEOUT_SECS` in
//! `db/src/auth/session.rs`), well inside its 90-day absolute TTL — a
//! long-idle MCP server must re-login unattended. The password and token are
//! never logged.

pub mod client;
pub mod config;
pub mod server;
pub mod tools;
