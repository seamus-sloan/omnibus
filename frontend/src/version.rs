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
    // A build-arg supplied without a value (e.g. `docker build` with no
    // `--build-arg OMNIBUS_VERSION=...`) sets the env var to an empty
    // string rather than leaving it unset, so `option_env!` alone would
    // treat that as "set" and render the bare "v". Filter it out here so
    // an empty/whitespace value falls back to CARGO_PKG_VERSION too.
    let raw = option_env!("OMNIBUS_VERSION")
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(env!("CARGO_PKG_VERSION"));
    normalize(raw)
}

/// Ensure `raw` has exactly one leading `v`, trimming whitespace and
/// collapsing any number of existing leading `v` characters (e.g. a doubled
/// `"vv0.8.9"`) down to one.
fn normalize(raw: &str) -> String {
    format!("v{}", raw.trim().trim_start_matches('v'))
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

    #[test]
    fn normalize_collapses_multiple_leading_v_and_trims_whitespace() {
        assert_eq!(normalize("  vv0.8.9\n"), "v0.8.9");
    }
}
