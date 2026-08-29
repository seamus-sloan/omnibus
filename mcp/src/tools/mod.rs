//! Tool implementations, one module per family. Each module contributes a
//! `#[tool_router(router = <name>)]` impl block on
//! [`crate::server::OmnibusMcp`]; `OmnibusMcp::new` combines the routers.
//! Read tools live in [`read`], the check-in / wishlist / physical-collection
//! family in [`checkin`], shelf authoring in [`shelves`]; later issues add
//! further families as sibling modules, subject to the allowlist policy in
//! [`crate::client`].

pub mod checkin;
pub mod read;
pub mod shelves;
