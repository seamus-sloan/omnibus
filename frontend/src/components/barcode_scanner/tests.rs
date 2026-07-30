//! Tests for `ScanStatus`: mapping the JS camera-glue status strings to the
//! enum (with an error default for unrecognized values), and which states
//! are terminal (the camera is out for good).

use super::*;

#[test]
fn from_glue_maps_every_known_state_and_defaults_to_error() {
    assert_eq!(ScanStatus::from_glue("starting"), ScanStatus::Starting);
    assert_eq!(ScanStatus::from_glue("ready"), ScanStatus::Ready);
    assert_eq!(ScanStatus::from_glue("denied"), ScanStatus::Denied);
    assert_eq!(
        ScanStatus::from_glue("unsupported"),
        ScanStatus::Unsupported
    );
    assert_eq!(ScanStatus::from_glue("wat"), ScanStatus::Error);
}

#[test]
fn is_terminal_is_true_only_once_the_camera_is_out_for_good() {
    assert!(!ScanStatus::Starting.is_terminal());
    assert!(!ScanStatus::Ready.is_terminal());
    assert!(ScanStatus::Denied.is_terminal());
    assert!(ScanStatus::Unsupported.is_terminal());
    assert!(ScanStatus::Error.is_terminal());
}

#[test]
fn every_terminal_status_points_the_reader_at_manual_entry() {
    for status in [
        ScanStatus::Denied,
        ScanStatus::Unsupported,
        ScanStatus::Error,
    ] {
        assert!(
            status.message().contains("ISBN"),
            "{status:?} must name the manual fallback"
        );
    }
}

#[test]
fn stage_attr_collapses_every_failure_to_off() {
    assert_eq!(scanner_state_attr(ScanStatus::Starting), "starting");
    assert_eq!(scanner_state_attr(ScanStatus::Ready), "ready");
    assert_eq!(scanner_state_attr(ScanStatus::Denied), "off");
    assert_eq!(scanner_state_attr(ScanStatus::Unsupported), "off");
    assert_eq!(scanner_state_attr(ScanStatus::Error), "off");
}
