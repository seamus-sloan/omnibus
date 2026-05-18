//! Shared auth-page primitives — the building blocks for [F1.6 Auth UI
//! polish](../../../../docs/roadmap/1-6-auth-ui.md).
//!
//! Each primitive is purely presentational: props in, rsx out. No signals,
//! no transport, no feature gating inside component bodies — SSR and WASM
//! must render identical markup so dioxus hydration matches.
//!
//! - [`AuthShell`] — split-pane wrapper used by every auth screen.
//! - [`Field`] — label + input + hint/error/success slots.
//! - [`Banner`] — top-of-form callout (err / warn / info / ok).
//! - [`StrengthMeter`] — four-segment presentational password strength bar.

mod banner;
mod field;
mod shell;
mod strength;

pub use banner::{Banner, BannerKind};
pub use field::Field;
pub use shell::AuthShell;
pub use strength::{StrengthMeter, StrengthScore};
