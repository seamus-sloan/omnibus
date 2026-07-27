use super::*;

#[test]
fn progress_update_rejects_cross_format_audio_field_on_epub() {
    let u = ProgressUpdate {
        book_uuid: "x".into(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
        audio_position_seconds: Some(12.0),
        progress_percent: None,
        kobo_location: None,
        client_updated_at: None,
    };
    let err = u
        .validate()
        .expect_err("epub payload must reject audio_position_seconds");
    assert!(err.contains("audio_position_seconds"), "got: {err}");
}

#[test]
fn progress_update_rejects_cross_format_cfi_on_audio() {
    let u = ProgressUpdate {
        book_uuid: "x".into(),
        format: ProgressFormat::Audio,
        epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
        audio_position_seconds: Some(12.0),
        progress_percent: None,
        kobo_location: None,
        client_updated_at: None,
    };
    let err = u
        .validate()
        .expect_err("audio payload must reject epub_cfi");
    assert!(err.contains("epub_cfi"), "got: {err}");
}

#[test]
fn progress_update_rejects_overlong_epub_cfi() {
    let u = ProgressUpdate {
        book_uuid: "x".into(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("a".repeat(EPUB_CFI_MAX_LEN + 1)),
        audio_position_seconds: None,
        progress_percent: None,
        kobo_location: None,
        client_updated_at: None,
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
        book_uuid: "x".into(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("é".repeat(EPUB_CFI_MAX_LEN)),
        audio_position_seconds: None,
        progress_percent: None,
        kobo_location: None,
        client_updated_at: None,
    };
    assert!(u.validate().is_ok());
}

#[test]
fn progress_update_rejects_negative_client_updated_at() {
    let u = ProgressUpdate {
        book_uuid: "x".into(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
        audio_position_seconds: None,
        progress_percent: None,
        kobo_location: None,
        client_updated_at: Some(-1),
    };
    let err = u
        .validate()
        .expect_err("negative client_updated_at must be rejected");
    assert!(err.contains("client_updated_at"), "got: {err}");
}

#[test]
fn progress_update_accepts_missing_client_updated_at() {
    let u = ProgressUpdate {
        book_uuid: "x".into(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
        audio_position_seconds: None,
        progress_percent: None,
        kobo_location: None,
        client_updated_at: None,
    };
    assert!(u.validate().is_ok(), "older clients must still validate");
}

#[test]
fn progress_update_accepts_percent_without_a_cfi_for_epub() {
    // The Kobo shape: a percent and an opaque location, no CFI. Requiring a
    // CFI here would lock the device out of the progress store entirely.
    let u = ProgressUpdate {
        book_uuid: "x".into(),
        format: ProgressFormat::Epub,
        epub_cfi: None,
        audio_position_seconds: None,
        progress_percent: Some(42),
        kobo_location: Some(r#"{"Type":"KoboSpan","Value":"kobo.9.1"}"#.into()),
        client_updated_at: None,
    };
    assert!(u.validate().is_ok(), "percent alone must satisfy epub");
}

#[test]
fn progress_update_rejects_epub_with_neither_cfi_nor_percent() {
    let u = ProgressUpdate {
        book_uuid: "x".into(),
        format: ProgressFormat::Epub,
        epub_cfi: None,
        audio_position_seconds: None,
        progress_percent: None,
        kobo_location: None,
        client_updated_at: None,
    };
    let err = u
        .validate()
        .expect_err("a positionless epub write must be rejected");
    assert!(err.contains("epub_cfi or progress_percent"), "got: {err}");
}

#[test]
fn progress_update_rejects_a_blank_cfi_with_no_percent() {
    // Whitespace is not a position — the old validator trimmed, and dropping
    // that would let `epub_cfi: "  "` through the relaxed branch.
    let u = ProgressUpdate {
        book_uuid: "x".into(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("   ".into()),
        audio_position_seconds: None,
        progress_percent: None,
        kobo_location: None,
        client_updated_at: None,
    };
    assert!(
        u.validate().is_err(),
        "blank cfi must not count as a position"
    );
}

#[test]
fn progress_update_rejects_a_blank_cfi_even_alongside_a_percent() {
    // The dangerous case: a percent satisfies the "some position" rule, so a
    // blank CFI would ride along — and `upsert_progress` merges with COALESCE,
    // where `Some("   ")` is non-NULL and overwrites a real stored anchor.
    let u = ProgressUpdate {
        book_uuid: "x".into(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("   ".into()),
        audio_position_seconds: None,
        progress_percent: Some(40),
        kobo_location: None,
        client_updated_at: None,
    };
    let err = u.validate().expect_err("blank cfi must be rejected");
    assert!(err.contains("epub_cfi"), "got: {err}");
}

#[test]
fn progress_update_rejects_out_of_range_percent() {
    for pct in [-1, 101] {
        let u = ProgressUpdate {
            book_uuid: "x".into(),
            format: ProgressFormat::Epub,
            epub_cfi: None,
            audio_position_seconds: None,
            progress_percent: Some(pct),
            kobo_location: None,
            client_updated_at: None,
        };
        let err = u.validate().expect_err("out-of-range percent must reject");
        assert!(err.contains("progress_percent"), "pct={pct} got: {err}");
    }
}

#[test]
fn progress_update_accepts_percent_at_both_bounds() {
    for pct in [0, 100] {
        let u = ProgressUpdate {
            book_uuid: "x".into(),
            format: ProgressFormat::Epub,
            epub_cfi: None,
            audio_position_seconds: None,
            progress_percent: Some(pct),
            kobo_location: None,
            client_updated_at: None,
        };
        assert!(u.validate().is_ok(), "pct={pct} must be accepted");
    }
}

#[test]
fn progress_update_rejects_kobo_fields_on_audio() {
    // The percent/location pair is epub-only; an audio row carries seconds.
    let base = ProgressUpdate {
        book_uuid: "x".into(),
        format: ProgressFormat::Audio,
        epub_cfi: None,
        audio_position_seconds: Some(12.0),
        progress_percent: None,
        kobo_location: None,
        client_updated_at: None,
    };
    let with_pct = ProgressUpdate {
        progress_percent: Some(50),
        ..base.clone()
    };
    assert!(with_pct
        .validate()
        .is_err_and(|e| e.contains("progress_percent")));
    let with_loc = ProgressUpdate {
        kobo_location: Some("{}".into()),
        ..base
    };
    assert!(with_loc
        .validate()
        .is_err_and(|e| e.contains("kobo_location")));
}

#[test]
fn progress_record_decodes_a_payload_predating_the_percent_fields() {
    // `#[serde(default)]` on the two new fields — a record from an older
    // server omits them entirely and must still decode.
    let json = r#"{
        "book_uuid": "u",
        "format": "epub",
        "epub_cfi": "epubcfi(/6/4)",
        "audio_position_seconds": null,
        "updated_at": 10,
        "client_updated_at": 10
    }"#;
    let rec: ProgressRecord = serde_json::from_str(json).expect("legacy payload must decode");
    assert_eq!(rec.progress_percent, None);
    assert_eq!(rec.kobo_location, None);
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
fn session_report_rejects_blank_client_id() {
    let r = SessionReport {
        book_uuid: "x".into(),
        format: ProgressFormat::Epub,
        started_at: 100,
        ended_at: 200,
        progress_units: 10,
        device_id: None,
        client_id: Some("   ".into()),
    };
    let err = r.validate().expect_err("a blank handle must be rejected");
    assert!(err.contains("client_id"), "got: {err}");
}

#[test]
fn session_report_rejects_overlong_client_id() {
    let r = SessionReport {
        book_uuid: "x".into(),
        format: ProgressFormat::Epub,
        started_at: 100,
        ended_at: 200,
        progress_units: 10,
        device_id: None,
        client_id: Some("a".repeat(SessionReport::CLIENT_ID_MAX_LEN + 1)),
    };
    let err = r
        .validate()
        .expect_err("an over-length handle must be rejected");
    assert!(err.contains("client_id"), "got: {err}");
}

#[test]
fn session_report_accepts_a_client_id_at_the_cap() {
    // Multibyte char: chars().count() == CLIENT_ID_MAX_LEN but len() (bytes)
    // is double that, so a regression to a byte-length check would fail this.
    let r = SessionReport {
        book_uuid: "x".into(),
        format: ProgressFormat::Epub,
        started_at: 100,
        ended_at: 200,
        progress_units: 10,
        device_id: None,
        client_id: Some("é".repeat(SessionReport::CLIENT_ID_MAX_LEN)),
    };
    assert!(r.validate().is_ok());
}

#[test]
fn session_report_accepts_a_missing_client_id() {
    // Web posts once on unmount and never retries, so it mints no handle.
    let r = SessionReport {
        book_uuid: "x".into(),
        format: ProgressFormat::Epub,
        started_at: 100,
        ended_at: 200,
        progress_units: 10,
        device_id: None,
        client_id: None,
    };
    assert!(r.validate().is_ok());
}
