//! Book / library / search / settings / overrides / worker fetchers. Each
//! function has a mobile REST variant (`reqwest`) and a web/SSR variant
//! (Dioxus server-function wrapper) with identical signatures across the
//! `#[cfg]` split. Split by sub-topic: [`admin`] (settings / library
//! sections / worker / scan), [`library`] (browse / search / manifests /
//! validators), [`manage`] (overrides / covers / merge / delete).

mod admin;
mod library;
mod manage;

pub use admin::*;
pub use library::*;
pub use manage::*;

#[cfg(all(test, feature = "mobile"))]
mod tests;
