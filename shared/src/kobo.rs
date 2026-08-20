//! Wire types for the per-device Kobo wireless-sync auth surface. A
//! `KoboDeviceView` is the re-displayable projection of a `kobo_devices` row
//! the Settings card renders — it deliberately carries the raw token so the
//! endpoint URL can be shown again (AC2). The token never leaves the owning
//! user's own authenticated response.

use serde::{Deserialize, Serialize};

/// Maximum byte length of a user-supplied Kobo device label.
pub const KOBO_DEVICE_NAME_MAX_LEN: usize = 100;

/// A registered Kobo as shown to its owner: the row id, its label, the
/// re-displayable path token, and last-seen bookkeeping. The hardware
/// `x-kobo-deviceid` and delta cursor stay server-side and are not projected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KoboDeviceView {
    pub id: i64,
    pub name: String,
    pub token: String,
    pub created_at: i64,
    pub last_seen_at: i64,
}

/// `true` once a trimmed device name is empty or exceeds
/// [`KOBO_DEVICE_NAME_MAX_LEN`]. Shared by the RPC boundary and any client
/// pre-check so both reject the same inputs.
pub fn kobo_device_name_invalid(name: &str) -> bool {
    let trimmed = name.trim();
    trimmed.is_empty() || trimmed.len() > KOBO_DEVICE_NAME_MAX_LEN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kobo_device_name_invalid_accepts_a_reasonable_label() {
        assert!(!kobo_device_name_invalid("Kobo Clara"));
    }

    #[test]
    fn kobo_device_name_invalid_rejects_an_empty_name() {
        assert!(kobo_device_name_invalid(""));
    }

    #[test]
    fn kobo_device_name_invalid_rejects_a_whitespace_only_name() {
        assert!(kobo_device_name_invalid("   \t\n  "));
    }

    #[test]
    fn kobo_device_name_invalid_rejects_a_name_over_the_max_length() {
        let name = "x".repeat(KOBO_DEVICE_NAME_MAX_LEN + 1);
        assert!(kobo_device_name_invalid(&name));
    }

    #[test]
    fn kobo_device_name_invalid_accepts_a_name_exactly_at_the_max_length() {
        let name = "x".repeat(KOBO_DEVICE_NAME_MAX_LEN);
        assert!(!kobo_device_name_invalid(&name));
    }

    #[test]
    fn kobo_device_name_invalid_trims_surrounding_whitespace_before_checking_length() {
        // Padding that would push the raw length over the cap should not
        // matter once trimmed.
        let padded = format!("  {}  ", "x".repeat(KOBO_DEVICE_NAME_MAX_LEN));
        assert!(!kobo_device_name_invalid(&padded));
    }
}
