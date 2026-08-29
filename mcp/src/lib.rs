//! Read-only MCP stdio server for Omnibus: authenticates over
//! `POST /api/auth/login` and exposes the library, search, discovery,
//! shelf, stats, progress, and annotation reads as MCP tools whose result
//! schemas derive from the `omnibus_shared` wire types. Write policy is
//! [`client::WRITE_ALLOWLIST`]; configuration is documented on [`config`].

pub mod client;
pub mod config;
pub mod server;
pub mod tools;
