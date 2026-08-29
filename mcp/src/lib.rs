//! MCP tool layer for Omnibus, served over two transports:
//!
//! * **stdio** (this crate's binary): authenticates over
//!   `POST /api/auth/login` (`--url`/`--username`/`--password` or the
//!   `OMNIBUS_MCP_*` env vars) against any instance.
//! * **hosted `/mcp`** (streamable HTTP, mounted by `server/` behind an
//!   admin toggle, default off): authenticates with an Omnibus API token —
//!   connect a client with
//!   `claude mcp add --transport http omnibus https://<host>/mcp --header
//!   "Authorization: Bearer <api-token>"`. Session bearers also work but
//!   idle-expire after 7 days; API tokens don't. The embedded service
//!   drives [`client::OmnibusClient::with_bearer`] against the server's own
//!   REST surface, so both transports share one tool layer and every
//!   permission gate is enforced by the same handlers.
//!
//! Tools expose the library, search, discovery, shelf, stats, progress, and
//! annotation reads — plus the check-in, wishlist, shelf, and metadata
//! repair tools — with result schemas derived from the `omnibus_shared`
//! wire types. Write policy is [`client::WRITE_ALLOWLIST`]; stdio
//! configuration is documented on [`config`].

pub mod client;
pub mod config;
pub mod server;
pub mod tools;
