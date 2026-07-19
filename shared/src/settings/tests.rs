//! Unit tests for the Settings wire-type validator.

use super::*;

fn ok_settings() -> Settings {
    Settings {
        ebook_library_path: Some("/lib/ebooks".into()),
        audiobook_library_path: Some("/lib/audio".into()),
        scan_interval_hours: None,
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
        scan_interval_hours: None,
    };
    assert!(s.validate().is_ok());
}

#[test]
fn settings_validate_accepts_ebook_path_at_max_len() {
    let s = Settings {
        ebook_library_path: Some("a".repeat(PATH_MAX_LEN)),
        audiobook_library_path: None,
        scan_interval_hours: None,
    };
    assert!(s.validate().is_ok());
}

#[test]
fn settings_validate_accepts_audiobook_path_at_max_len() {
    let s = Settings {
        ebook_library_path: None,
        audiobook_library_path: Some("a".repeat(PATH_MAX_LEN)),
        scan_interval_hours: None,
    };
    assert!(s.validate().is_ok());
}

#[test]
fn settings_validate_rejects_ebook_path_over_max_len() {
    let s = Settings {
        ebook_library_path: Some("a".repeat(PATH_MAX_LEN + 1)),
        audiobook_library_path: None,
        scan_interval_hours: None,
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
        scan_interval_hours: None,
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
fn kindle_email_oversize_is_false_at_and_below_the_cap() {
    // The cap itself fits (strictly-greater comparison), as does a small file.
    assert!(!kindle_email_oversize(0));
    assert!(!kindle_email_oversize(KINDLE_EMAIL_MAX_BYTES));
}

#[test]
fn kindle_email_oversize_is_true_one_byte_over_the_cap() {
    assert!(kindle_email_oversize(KINDLE_EMAIL_MAX_BYTES + 1));
}

#[test]
fn kindle_size_limits_match_amazons_documented_figures() {
    // Email = 50 MB, web uploader = 200 MB, expressed as decimal megabytes to
    // match Amazon's user-facing figures and the UI copy. Guards against an
    // accidental unit slip (decimal MB vs binary MiB, or a wrong multiplier)
    // since both feed user-facing copy and the send guard.
    assert_eq!(KINDLE_EMAIL_MAX_BYTES, 50_000_000);
    assert_eq!(KINDLE_WEB_MAX_BYTES, 200_000_000);
}

#[test]
fn settings_validate_reports_ebook_field_when_both_are_over_max_len() {
    // Ebook is checked first; the error must name it rather than the
    // audiobook field so the caller surfaces the right form field.
    let s = Settings {
        ebook_library_path: Some("a".repeat(PATH_MAX_LEN + 1)),
        audiobook_library_path: Some("b".repeat(PATH_MAX_LEN + 1)),
        scan_interval_hours: None,
    };
    let err = s.validate().expect_err("over-long path must be rejected");
    assert_eq!(
        err,
        SettingsError::PathTooLong {
            field: "ebook_library_path"
        }
    );
}

#[test]
fn is_valid_metadata_precedence_accepts_the_default_order() {
    assert!(is_valid_metadata_precedence(&DEFAULT_METADATA_PRECEDENCE));
}

#[test]
fn is_valid_metadata_precedence_accepts_any_permutation() {
    let reordered = [
        MetadataSource::OmnibusOverrides,
        MetadataSource::EmbeddedTags,
        MetadataSource::FolderStructure,
        MetadataSource::ProviderMatch,
        MetadataSource::OpfSidecar,
    ];
    assert!(is_valid_metadata_precedence(&reordered));
}

#[test]
fn is_valid_metadata_precedence_rejects_a_short_list() {
    assert!(!is_valid_metadata_precedence(&[
        MetadataSource::EmbeddedTags,
        MetadataSource::OmnibusOverrides,
    ]));
}

#[test]
fn is_valid_metadata_precedence_rejects_a_duplicate_entry() {
    let dup = [
        MetadataSource::EmbeddedTags,
        MetadataSource::EmbeddedTags,
        MetadataSource::FolderStructure,
        MetadataSource::OmnibusOverrides,
        MetadataSource::ProviderMatch,
    ];
    assert!(!is_valid_metadata_precedence(&dup));
}

#[test]
fn metadata_source_serializes_to_snake_case_json() {
    let json = serde_json::to_string(&DEFAULT_METADATA_PRECEDENCE).unwrap();
    assert_eq!(
        json,
        r#"["folder_structure","embedded_tags","opf_sidecar","omnibus_overrides","provider_match"]"#
    );
}

#[test]
fn settings_validate_accepts_scan_interval_at_the_minimum() {
    let s = Settings {
        ebook_library_path: None,
        audiobook_library_path: None,
        scan_interval_hours: Some(SCAN_INTERVAL_MIN_HOURS),
    };
    assert!(s.validate().is_ok());
}

#[test]
fn settings_validate_rejects_scan_interval_of_zero() {
    let s = Settings {
        ebook_library_path: None,
        audiobook_library_path: None,
        scan_interval_hours: Some(0),
    };
    assert_eq!(
        s.validate().expect_err("a zero interval must be rejected"),
        SettingsError::ScanIntervalTooSmall
    );
}
