//! Server library — hand-written `/api/*` REST routes for mobile.
//!
//! The unified Dioxus fullstack binary lives in `main.rs`. This lib crate
//! exists so `backend`'s integration tests can import it with `use omnibus::backend`.

#[cfg(feature = "server")]
pub mod auth;
#[cfg(feature = "server")]
pub mod backend;
#[cfg(feature = "server")]
pub mod worker;
