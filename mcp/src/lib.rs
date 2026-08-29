//! MCP stdio server for Omnibus: authenticates over `POST /api/auth/login`
//! and exposes the library, search, discovery, shelf, stats, progress, and
//! annotation reads — plus the physical check-in and wishlist tools — with
//! result schemas derived from the `omnibus_shared` wire types. Write policy
//! is [`client::WRITE_ALLOWLIST`]; configuration is documented on [`config`].

pub mod client;
pub mod config;
pub mod server;
pub mod tools;
