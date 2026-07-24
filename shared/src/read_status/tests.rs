use super::*;

#[test]
fn read_status_round_trips_through_str_tokens() {
    for s in [
        ReadStatus::Unread,
        ReadStatus::Reading,
        ReadStatus::Finished,
    ] {
        assert_eq!(ReadStatus::from_str(s.as_str()), Some(s));
    }
}

#[test]
fn from_str_rejects_unknown_token() {
    assert_eq!(ReadStatus::from_str("done"), None);
}

#[test]
fn from_db_defaults_unknown_to_unread() {
    assert_eq!(ReadStatus::from_db("garbage"), ReadStatus::Unread);
    assert_eq!(ReadStatus::from_db("finished"), ReadStatus::Finished);
}

#[test]
fn default_read_status_is_unread() {
    assert_eq!(ReadStatus::default(), ReadStatus::Unread);
}

#[test]
fn set_read_status_serializes_status_as_lowercase() {
    let json = serde_json::to_string(&SetReadStatus {
        book_uuid: "abc".into(),
        status: ReadStatus::Finished,
    })
    .unwrap();
    assert!(json.contains("\"finished\""), "json was {json}");
}

#[test]
fn set_read_status_validate_rejects_blank_uuid() {
    let req = SetReadStatus {
        book_uuid: "   ".into(),
        status: ReadStatus::Reading,
    };
    assert!(req.validate().is_err());
}

#[test]
fn set_read_status_validate_accepts_nonblank_uuid() {
    let req = SetReadStatus {
        book_uuid: "book-1".into(),
        status: ReadStatus::Reading,
    };
    assert!(req.validate().is_ok());
}

#[test]
fn record_omits_null_finished_at_from_wire() {
    let json = serde_json::to_string(&ReadStatusRecord {
        book_uuid: "abc".into(),
        status: ReadStatus::Reading,
        updated_at: 10,
        finished_at: None,
    })
    .unwrap();
    assert!(!json.contains("finished_at"), "json was {json}");
}
