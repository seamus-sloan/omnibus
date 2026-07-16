//! Compile-time app version. Release CI stamps `OMNIBUS_VERSION` into the
//! build environment (see `.github/workflows/docker.yml` and
//! `testflight.yml`); local dev builds fall back to `CARGO_PKG_VERSION` (the
//! crate version, pinned at `0.1.0`) so `dx serve` / `cargo build` keep
//! compiling and rendering a version string without the env var set.

/// This build's own version string, always with a single leading `v` (e.g.
/// `v0.8.9`; `v0.1.0` for a local dev build with no `OMNIBUS_VERSION` set —
/// AC6). Docker's build-arg carries the full release tag (already
/// `v`-prefixed, see `Dockerfile`), while TestFlight's carries the bare
/// `MARKETING_VERSION` Apple requires (no `v`) — [`normalize`] tolerates
/// either so callers get one consistent format regardless of source.
pub fn app_version() -> String {
    normalize(option_env!("OMNIBUS_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")))
}

/// Ensure `raw` has exactly one leading `v`.
fn normalize(raw: &str) -> String {
    if let Some(stripped) = raw.strip_prefix('v') {
        format!("v{stripped}")
    } else {
        format!("v{raw}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_version_falls_back_to_v_prefixed_cargo_pkg_version_locally() {
        // OMNIBUS_VERSION isn't set for `cargo test`, so this pins the local
        // dev fallback (AC6): "v" + CARGO_PKG_VERSION, currently "v0.1.0".
        assert_eq!(app_version(), format!("v{}", env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn normalize_adds_a_leading_v_when_absent() {
        assert_eq!(normalize("0.8.9"), "v0.8.9");
    }

    #[test]
    fn normalize_does_not_double_an_existing_leading_v() {
        assert_eq!(normalize("v0.8.9"), "v0.8.9");
    }
}
