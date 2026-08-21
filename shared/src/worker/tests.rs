use super::*;

#[test]
fn done_without_a_ghost_warning_omits_the_field_from_the_wire() {
    // AC5: the wire type must stay compact for the ordinary (no
    // warning) case — no `ghost_warning` key at all, not even `null`.
    let state = ProgressState::Done {
        processed: 3,
        ghost_warning: None,
        bake_errors: None,
    };
    let json = serde_json::to_string(&state).unwrap();
    assert!(
        !json.contains("ghost_warning"),
        "expected no ghost_warning key, got {json}"
    );
}

#[test]
fn done_with_a_ghost_warning_round_trips_the_removed_and_total_counts() {
    let state = ProgressState::Done {
        processed: 100,
        ghost_warning: Some(GhostFilesWarning {
            removed: 15,
            total: 100,
        }),
        bake_errors: None,
    };
    let json = serde_json::to_string(&state).unwrap();
    let round_tripped: ProgressState = serde_json::from_str(&json).unwrap();
    assert_eq!(state, round_tripped);
}

#[test]
fn done_without_bake_errors_omits_the_field_from_the_wire() {
    // Mirrors `done_without_a_ghost_warning_omits_the_field_from_the_wire`
    // (#1739): the wire type must stay compact for the ordinary
    // all-succeeded bake case too.
    let state = ProgressState::Done {
        processed: 3,
        ghost_warning: None,
        bake_errors: None,
    };
    let json = serde_json::to_string(&state).unwrap();
    assert!(
        !json.contains("bake_errors"),
        "expected no bake_errors key, got {json}"
    );
}

#[test]
fn done_with_bake_errors_round_trips_the_book_uuids() {
    let state = ProgressState::Done {
        processed: 2,
        ghost_warning: None,
        bake_errors: Some(vec!["uuid-bad".into()]),
    };
    let json = serde_json::to_string(&state).unwrap();
    let round_tripped: ProgressState = serde_json::from_str(&json).unwrap();
    assert_eq!(state, round_tripped);
}

#[test]
fn task_progress_without_detail_omits_the_field_from_the_wire() {
    let progress = TaskProgress {
        task_id: 1,
        kind: TaskKind::Scan,
        state: ProgressState::Running {
            processed: 0,
            total: None,
        },
        resource_key: None,
        detail: None,
        started_at_ms: 0,
        last_update_ms: 0,
    };
    let json = serde_json::to_string(&progress).unwrap();
    assert!(
        !json.contains("detail"),
        "expected no detail key, got {json}"
    );
}

#[test]
fn task_progress_with_detail_round_trips_phase_item_and_tallies() {
    let progress = TaskProgress {
        task_id: 1,
        kind: TaskKind::Scan,
        state: ProgressState::Running {
            processed: 3,
            total: Some(10),
        },
        resource_key: None,
        detail: Some(TaskDetail {
            phase: Some("Reading file metadata".into()),
            current_item: Some("books/Author/Title.epub".into()),
            tallies: Some(ScanTallies {
                found: 10,
                new: 3,
                changed: 1,
                removed: 2,
                moved: 1,
                unchanged: 3,
            }),
        }),
        started_at_ms: 0,
        last_update_ms: 0,
    };
    let json = serde_json::to_string(&progress).unwrap();
    let round_tripped: TaskProgress = serde_json::from_str(&json).unwrap();
    assert_eq!(progress, round_tripped);
}

#[test]
fn task_detail_is_empty_only_when_every_field_is_absent() {
    assert!(TaskDetail::default().is_empty());
    assert!(!TaskDetail {
        phase: Some("Walking the library".into()),
        ..Default::default()
    }
    .is_empty());
}

#[test]
fn done_with_bake_errors_never_carries_a_message_key_on_the_wire() {
    // Regression guard for the PR #1756 review fix: the wire payload
    // must be uuid-only — no per-book error text, which can carry a
    // server filesystem path and is reachable by any authenticated user
    // via `rpc_worker_status`, not only the admin who ran the bake.
    let state = ProgressState::Done {
        processed: 1,
        ghost_warning: None,
        bake_errors: Some(vec!["uuid-bad".into()]),
    };
    let json = serde_json::to_string(&state).unwrap();
    assert!(
        !json.contains("message"),
        "bake_errors must not carry a message field: {json}"
    );
}
