//! Tool implementations, one module per family. Each module contributes a
//! `#[tool_router(router = <name>)]` impl block on
//! [`crate::server::OmnibusMcp`]; `OmnibusMcp::new` combines the routers.
//! Read tools live in [`read`]; later issues add write families as sibling
//! modules, subject to the allowlist policy in [`crate::client`].

pub mod read;
