//! Unit tests for the Settings wire-type validator.

use super::*;

fn ok_settings() -> Settings {
    Settings {
        ebook_library_path: Some("/lib/ebooks".into()),
        audiobook_library_path: Some("/lib/audio".into()),
    }
}

#[test]
fn settings_validate_accepts_short_paths() {
    assert!(ok_settings().validate().is_ok());
}

#[test]
fn settings_validate_accepts_none_paths() {
    let s = Settings {
        ebook_library_path: None,
        audiobook_library_path: None,
    };
    assert!(s.validate().is_ok());
}

#[test]
fn settings_validate_accepts_ebook_path_at_max_len() {
    let s = Settings {
        ebook_library_path: Some("a".repeat(PATH_MAX_LEN)),
        audiobook_library_path: None,
    };
    assert!(s.validate().is_ok());
}

#[test]
fn settings_validate_accepts_audiobook_path_at_max_len() {
    let s = Settings {
        ebook_library_path: None,
        audiobook_library_path: Some("a".repeat(PATH_MAX_LEN)),
    };
    assert!(s.validate().is_ok());
}

#[test]
fn settings_validate_rejects_ebook_path_over_max_len() {
    let s = Settings {
        ebook_library_path: Some("a".repeat(PATH_MAX_LEN + 1)),
        audiobook_library_path: None,
    };
    let err = s
        .validate()
        .expect_err("over-long ebook path must be rejected");
    assert_eq!(
        err,
        SettingsError::PathTooLong {
            field: "ebook_library_path"
        }
    );
    let msg = err.to_string();
    assert!(msg.contains("ebook_library_path"), "got: {msg}");
    assert!(msg.contains(&PATH_MAX_LEN.to_string()), "got: {msg}");
}

#[test]
fn settings_validate_rejects_audiobook_path_over_max_len() {
    let s = Settings {
        ebook_library_path: None,
        audiobook_library_path: Some("a".repeat(PATH_MAX_LEN + 1)),
    };
    let err = s
        .validate()
        .expect_err("over-long audiobook path must be rejected");
    assert_eq!(
        err,
        SettingsError::PathTooLong {
            field: "audiobook_library_path"
        }
    );
    let msg = err.to_string();
    assert!(msg.contains("audiobook_library_path"), "got: {msg}");
    assert!(msg.contains(&PATH_MAX_LEN.to_string()), "got: {msg}");
}

#[test]
fn settings_validate_reports_ebook_field_when_both_are_over_max_len() {
    // Ebook is checked first; the error must name it rather than the
    // audiobook field so the caller surfaces the right form field.
    let s = Settings {
        ebook_library_path: Some("a".repeat(PATH_MAX_LEN + 1)),
        audiobook_library_path: Some("b".repeat(PATH_MAX_LEN + 1)),
    };
    let err = s.validate().expect_err("over-long path must be rejected");
    assert_eq!(
        err,
        SettingsError::PathTooLong {
            field: "ebook_library_path"
        }
    );
}
