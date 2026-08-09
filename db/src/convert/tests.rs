use crate::test_support::EnvVarGuard;

use super::*;

#[test]
fn ebook_convert_bin_returns_the_env_override_when_set() {
    let _guard = EnvVarGuard::set(
        "OMNIBUS_EBOOK_CONVERT_PATH",
        Some("/opt/calibre/ebook-convert"),
    );
    assert_eq!(ebook_convert_bin(), "/opt/calibre/ebook-convert");
}

#[test]
fn ebook_convert_bin_falls_back_to_the_bare_name_when_the_env_var_is_unset() {
    let _guard = EnvVarGuard::set("OMNIBUS_EBOOK_CONVERT_PATH", None);
    assert_eq!(ebook_convert_bin(), "ebook-convert");
}

#[test]
fn ebook_convert_bin_treats_a_blank_env_override_as_unset() {
    let _guard = EnvVarGuard::set("OMNIBUS_EBOOK_CONVERT_PATH", Some("   "));
    assert_eq!(ebook_convert_bin(), "ebook-convert");
}

#[test]
fn is_runnable_returns_false_for_a_binary_that_does_not_exist() {
    assert!(!is_runnable("/nonexistent/omnibus-ebook-convert-probe"));
}

#[test]
fn is_runnable_returns_true_for_a_binary_that_exits_zero() {
    // `env --version` exits 0 on every platform the server targets, so it
    // stands in for a working Calibre install without needing one.
    assert!(is_runnable("env"));
}

#[test]
fn ebook_convert_available_returns_false_when_the_configured_binary_is_missing() {
    let _guard = EnvVarGuard::set(
        "OMNIBUS_EBOOK_CONVERT_PATH",
        Some("/nonexistent/omnibus-ebook-convert-probe"),
    );
    assert!(!ebook_convert_available());
}

#[test]
fn warn_if_unavailable_does_not_panic_when_the_binary_is_missing() {
    let _guard = EnvVarGuard::set(
        "OMNIBUS_EBOOK_CONVERT_PATH",
        Some("/nonexistent/omnibus-ebook-convert-probe"),
    );
    warn_if_unavailable();
}
