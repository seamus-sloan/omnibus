//! Shared auth-page primitives used by [`crate::pages::auth`]: [`AuthShell`]
//! split-pane wrapper, [`Field`] label+input+hint slots, [`Banner`] callout,
//! and [`StrengthMeter`] password bar. Purely presentational (props in,
//! rsx out) with no feature-gated bodies so SSR and WASM markup matches.

mod banner;
mod field;
mod shell;
mod strength;

pub use banner::{Banner, BannerKind};
pub use field::Field;
pub use shell::AuthShell;
pub use strength::{score_password, PasswordRequirements, StrengthMeter, StrengthScore};
