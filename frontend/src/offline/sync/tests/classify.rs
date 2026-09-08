//! The error classifier and the watch state: connect failures count as
//! offline, decode / HTTP / unauthorized errors do not, the fast-fail
//! variant is accepted, and the offline/online and dropped-count notes
//! flip and accumulate.

#![allow(clippy::await_holding_lock)]

use super::super::*;
use crate::offline::test_support::{connect_refused_error, decode_error};

#[tokio::test]
async fn is_offline_error_accepts_connect_failures() {
    let e = connect_refused_error().await;
    assert!(
        is_offline_error(&e),
        "connect-refused must classify offline"
    );
}

#[tokio::test]
async fn is_offline_error_rejects_decode_failures() {
    let e = decode_error().await;
    assert!(
        matches!(e, DataError::Network(_)),
        "mobile decode failures surface as Network"
    );
    assert!(
        !is_offline_error(&e),
        "decode failure means the server WAS reachable"
    );
}

#[test]
fn is_offline_error_rejects_http_and_unauthorized() {
    assert!(!is_offline_error(&DataError::Http {
        status: 500,
        body: String::new()
    }));
    assert!(!is_offline_error(&DataError::Unauthorized));
    assert!(!is_offline_error(&DataError::Other("x".into())));
}

#[test]
fn is_offline_error_accepts_the_fast_fail_offline_variant() {
    assert!(
        is_offline_error(&DataError::Offline),
        "a precheck fast-fail is the same class as a failed connect"
    );
}

#[test]
fn note_offline_and_note_online_flip_the_watch_state() {
    let _guard = test_state_lock().lock().unwrap();
    note_offline();
    assert!(is_offline());
    assert!(!state().online);
    note_online();
    assert!(!is_offline());
    assert!(state().online);
}

#[test]
fn note_dropped_accumulates_only_nonzero_counts() {
    let _guard = test_state_lock().lock().unwrap();
    let before = state().dropped_ops;
    note_dropped(0);
    assert_eq!(state().dropped_ops, before);
    note_dropped(2);
    assert_eq!(state().dropped_ops, before + 2);
}
