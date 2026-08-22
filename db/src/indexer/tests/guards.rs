//! The data-loss guards around a scan: `enumeration_trustworthy`, the
//! mass-missing abort check, and the ghost-warning threshold the reindex
//! stats carry.

use super::super::*;

#[test]
fn enumeration_trustworthy_true_for_healthy_populated_scan() {
    // Complete walk, files present (saw_any_file), DB has files — normal case.
    assert!(enumeration_is_trustworthy(false, true, true));
}

#[test]
fn enumeration_untrustworthy_when_incomplete() {
    // A subdir read failed — partial view, distrust regardless of the rest.
    assert!(!enumeration_is_trustworthy(true, true, true));
}

#[test]
fn enumeration_untrustworthy_when_populated_root_reads_totally_empty() {
    // The boot-race / unmounted-NFS case: the walk saw NO file of any
    // extension but the DB still holds file-backed books. Distrust so the
    // removal pass is skipped.
    assert!(!enumeration_is_trustworthy(false, false, true));
}

#[test]
fn enumeration_trustworthy_when_root_has_files_of_another_format() {
    // #328 shared-path: the root has files (saw_any_file), just none of this
    // library's format. That is a legitimate empty diff, not a fault — trust
    // it so the cross-format removal still works.
    assert!(enumeration_is_trustworthy(false, true, true));
}

#[test]
fn enumeration_trustworthy_when_empty_root_matches_empty_db() {
    // A genuinely empty library (or first-ever scan) must stay indexable —
    // an empty read is only suspicious when the DB disagrees.
    assert!(enumeration_is_trustworthy(false, false, false));
}

#[test]
fn check_mass_missing_allows_removals_within_the_threshold() {
    // 20 of 100 (20%) is at the boundary, not over it — allowed.
    assert!(check_mass_missing(20, 100).is_ok());
    // 21 of 100 (21%) trips the breaker.
    assert!(check_mass_missing(21, 100).is_err());
}

#[test]
fn check_mass_missing_allows_small_absolute_removals_regardless_of_percent() {
    // Deleting the only book in a 1-book library is 100% but under the
    // absolute floor, so it must not trip the breaker.
    assert!(check_mass_missing(1, 1).is_ok());
    assert!(check_mass_missing(MASS_MISSING_MIN_ABSOLUTE, MASS_MISSING_MIN_ABSOLUTE).is_ok());
}

#[test]
fn check_mass_missing_reports_counts_and_percent_in_the_error() {
    let err = check_mass_missing(50, 100).unwrap_err();
    assert_eq!(err.removed, 50);
    assert_eq!(err.total, 100);
    assert!((err.percent - 50.0).abs() < f64::EPSILON);
}

// ---------- #1057: warn threshold (the sub-abort middle ground) ----------

#[test]
fn ghost_warning_threshold_exceeded_is_false_below_the_warn_fraction() {
    // 10 of 100 (10%) sits at the warn boundary, not over it — silent.
    assert!(!ghost_warning_threshold_exceeded(10, 100));
    // 9 of 100 (9%) is comfortably under the warn fraction.
    assert!(!ghost_warning_threshold_exceeded(9, 100));
}

#[test]
fn ghost_warning_threshold_exceeded_is_false_under_the_absolute_floor_regardless_of_percent() {
    // Deleting the only book in a 1-book library is 100% but under the
    // absolute floor shared with the abort guard — never warns.
    assert!(!ghost_warning_threshold_exceeded(1, 1));
    assert!(!ghost_warning_threshold_exceeded(
        MASS_MISSING_MIN_ABSOLUTE,
        MASS_MISSING_MIN_ABSOLUTE
    ));
}

#[test]
fn ghost_warning_threshold_exceeded_is_false_when_the_library_has_no_file_backed_books() {
    assert!(!ghost_warning_threshold_exceeded(50, 0));
}

#[test]
fn ghost_warning_threshold_exceeded_is_true_in_the_warn_band_below_the_abort_guard() {
    // 15 of 100 (15%) clears the 10% warn fraction but stays under the 20%
    // abort fraction — the sub-abort middle ground this issue adds.
    assert!(ghost_warning_threshold_exceeded(15, 100));
    assert!(
        check_mass_missing(15, 100).is_ok(),
        "the warn band must not trip the #819 abort guard"
    );
}

#[test]
fn ghost_warning_threshold_exceeded_is_true_up_to_the_abort_boundary() {
    // 20 of 100 (20%) is the abort guard's own boundary (still allowed
    // through by check_mass_missing) and clears the warn fraction too.
    assert!(ghost_warning_threshold_exceeded(20, 100));
    assert!(check_mass_missing(20, 100).is_ok());
    // 21 of 100 (21%) crosses into the abort guard.
    assert!(check_mass_missing(21, 100).is_err());
}

#[test]
fn reindex_stats_ghost_warning_is_none_below_the_warn_threshold() {
    let stats = ReindexStats {
        removed: 5,
        file_backed_total: 100,
        ..Default::default()
    };
    assert_eq!(stats.ghost_warning(), None);
}

#[test]
fn reindex_stats_ghost_warning_carries_the_removed_and_total_counts_in_the_warn_band() {
    let stats = ReindexStats {
        removed: 15,
        file_backed_total: 100,
        ..Default::default()
    };
    assert_eq!(
        stats.ghost_warning(),
        Some(omnibus_shared::GhostFilesWarning {
            removed: 15,
            total: 100,
        })
    );
}
