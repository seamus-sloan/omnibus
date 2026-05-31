//! Shared auth-page primitives. Each is purely presentational: props in,
//! rsx out. No signals, no transport, no feature gating inside component
//! bodies — SSR and WASM must render identical markup so dioxus hydration
//! matches. Provides [`AuthShell`] (split-pane wrapper), [`Field`]
//! (label + input + hint/error/success slots), [`Banner`] (callout), and
//! [`StrengthMeter`] (four-segment password strength bar).

mod banner;
mod field;
mod shell;
mod strength;

pub use banner::{Banner, BannerKind};
pub use field::Field;
pub use shell::AuthShell;
pub use strength::{StrengthMeter, StrengthScore};
