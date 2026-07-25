use super::*;

#[test]
fn progress_update_rejects_cross_format_audio_field_on_epub() {
    let u = ProgressUpdate {
        client_updated_at: None,
        book_uuid: "x".into(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
        audio_position_seconds: Some(12.0),
    };
    let err = u
        .validate()
        .expect_err("epub payload must reject audio_position_seconds");
    assert!(err.contains("audio_position_seconds"), "got: {err}");
}

#[test]
fn progress_update_rejects_cross_format_cfi_on_audio() {
    let u = ProgressUpdate {
        client_updated_at: None,
        book_uuid: "x".into(),
        format: ProgressFormat::Audio,
        epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
        audio_position_seconds: Some(12.0),
    };
    let err = u
        .validate()
        .expect_err("audio payload must reject epub_cfi");
    assert!(err.contains("epub_cfi"), "got: {err}");
}

#[test]
fn progress_update_rejects_overlong_epub_cfi() {
    let u = ProgressUpdate {
        client_updated_at: None,
        book_uuid: "x".into(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("a".repeat(EPUB_CFI_MAX_LEN + 1)),
        audio_position_seconds: None,
    };
    let err = u
        .validate()
        .expect_err("over-cap epub_cfi must be rejected");
    assert!(err.contains("epub_cfi"), "got: {err}");
}

#[test]
fn progress_update_accepts_epub_cfi_at_cap() {
    // Multibyte char: chars().count() == EPUB_CFI_MAX_LEN but len() (bytes)
    // is double that, so a regression to a byte-length check would fail this.
    let u = ProgressUpdate {
        client_updated_at: None,
        book_uuid: "x".into(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("é".repeat(EPUB_CFI_MAX_LEN)),
        audio_position_seconds: None,
    };
    assert!(u.validate().is_ok());
}

#[test]
fn session_report_rejects_inverted_time_range() {
    let r = SessionReport {
        book_uuid: "x".into(),
        format: ProgressFormat::Epub,
        started_at: 500,
        ended_at: 200,
        progress_units: 0,
        device_id: None,
        client_id: None,
    };
    let err = r.validate().expect_err("ended < started must be rejected");
    assert!(err.contains("ended_at"), "got: {err}");
}

#[test]
fn session_report_rejects_negative_progress_units() {
    let r = SessionReport {
        book_uuid: "x".into(),
        format: ProgressFormat::Audio,
        started_at: 100,
        ended_at: 200,
        progress_units: -5,
        device_id: None,
        client_id: None,
    };
    let err = r
        .validate()
        .expect_err("negative progress_units must be rejected");
    assert!(err.contains("progress_units"), "got: {err}");
}

#[test]
fn progress_update_rejects_a_negative_client_clock() {
    let u = ProgressUpdate {
        book_uuid: "b".into(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("epubcfi(/6/4)".into()),
        audio_position_seconds: None,
        client_updated_at: Some(-1),
    };
    assert!(u.validate().is_err());
}

#[test]
fn progress_update_accepts_a_missing_client_clock() {
    let u = ProgressUpdate {
        book_uuid: "b".into(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("epubcfi(/6/4)".into()),
        audio_position_seconds: None,
        client_updated_at: None,
    };
    assert!(u.validate().is_ok());
}
