//! Tests for the admin server-health page: the pure formatters, and a
//! render assertion per section (rendered output contains the expected
//! content, per `.claude/rules/03-unit-testing.md`'s frontend guidance).

use omnibus_shared::worker::TaskKind;

use super::*;

// ---------- pure formatters ----------

#[test]
fn format_bytes_scales_the_unit_to_the_byte_count() {
    assert_eq!(format_bytes(0), "0 B");
    assert_eq!(format_bytes(512), "512 B");
    assert_eq!(format_bytes(3_100), "3.1 KB");
    assert_eq!(format_bytes(3_100_000), "3.1 MB");
    assert_eq!(format_bytes(2_500_000_000), "2.5 GB");
}

#[test]
fn format_cache_usage_joins_the_used_and_cap_figures() {
    assert_eq!(format_cache_usage(0, 0), "0 B / 0 B cap");
    assert_eq!(
        format_cache_usage(3_100_000, 5_000_000_000),
        "3.1 MB / 5.0 GB cap"
    );
}

#[test]
fn path_or_unconfigured_reports_the_path_when_present() {
    assert_eq!(path_or_unconfigured(Some("/library")), "/library");
}

#[test]
fn path_or_unconfigured_falls_back_when_absent_or_blank() {
    assert_eq!(path_or_unconfigured(None), "Not configured");
    assert_eq!(path_or_unconfigured(Some("   ")), "Not configured");
}

#[test]
fn fmt_timestamp_formats_a_known_epoch_second() {
    assert_eq!(fmt_timestamp(1_700_000_000), "2023-11-14 22:13:20 UTC");
}

#[test]
fn fmt_timestamp_formats_the_epoch_itself() {
    assert_eq!(fmt_timestamp(0), "1970-01-01 00:00:00 UTC");
}

// ---------- render tests (SSR markup) ----------

fn index_status() -> IndexStatus {
    IndexStatus {
        ebook_library_path: Some("/lib/ebooks".into()),
        audiobook_library_path: None,
        book_count: 42,
        author_count: 7,
        series_count: 3,
        ghosted_book_count: 0,
    }
}

#[test]
fn index_status_card_renders_paths_and_counts() {
    let html = dioxus::ssr::render_element(rsx! { IndexStatusCard { status: index_status() } });
    assert!(html.contains("data-testid=\"admin-health-index-card\""));
    assert!(html.contains("/lib/ebooks"));
    assert!(html.contains("Not configured"));
    assert!(html.contains("42"));
    assert!(html.contains("data-testid=\"admin-health-ghosted-ok\""));
    assert!(!html.contains("admin-health-ghosted-warning"));
}

#[test]
fn index_status_card_warns_when_books_are_ghosted() {
    let mut status = index_status();
    status.ghosted_book_count = 2;
    let html = dioxus::ssr::render_element(rsx! { IndexStatusCard { status } });
    assert!(html.contains("data-testid=\"admin-health-ghosted-warning\""));
    assert!(html.contains("2 book(s) have no file on disk"));
}

#[test]
fn worker_queue_card_renders_empty_state_when_no_entries() {
    let html = dioxus::ssr::render_element(rsx! { WorkerQueueCard { entries: Vec::new() } });
    assert!(html.contains("data-testid=\"admin-health-worker-empty\""));
    assert!(!html.contains("admin-health-worker-table"));
}

#[test]
fn worker_queue_card_renders_a_row_per_entry() {
    let entries = vec![WorkerQueueEntry {
        kind: TaskKind::Scan,
        queue_depth: 1,
        recent_completions_ms: vec![120, 340],
    }];
    let html = dioxus::ssr::render_element(rsx! { WorkerQueueCard { entries } });
    assert!(html.contains("data-testid=\"admin-health-worker-table\""));
    assert!(html.contains("Scanning library"));
    assert!(html.contains("340 ms"));
}

#[test]
fn fts_health_card_reports_in_sync() {
    let fts = FtsHealth {
        books_rows: 10,
        fts_rows: 10,
        in_sync: true,
    };
    let html = dioxus::ssr::render_element(rsx! { FtsHealthCard { fts } });
    assert!(html.contains("data-testid=\"admin-health-fts-in-sync\""));
    assert!(!html.contains("admin-health-fts-out-of-sync"));
}

#[test]
fn fts_health_card_reports_out_of_sync() {
    let fts = FtsHealth {
        books_rows: 10,
        fts_rows: 4,
        in_sync: false,
    };
    let html = dioxus::ssr::render_element(rsx! { FtsHealthCard { fts } });
    assert!(html.contains("data-testid=\"admin-health-fts-out-of-sync\""));
    assert!(html.contains("rebuild is recommended"));
}

#[test]
fn storage_card_renders_every_figure() {
    let storage = StorageStats {
        library_bytes: 3_100_000,
        covers_bytes: 512,
        thumbs_bytes: 1_000,
        thumbs_cap_bytes: 5_000_000_000,
        hls_bytes: 0,
        hls_cap_bytes: 5_000_000_000,
        export_epub_bytes: 2_000,
        export_epub_cap_bytes: 5_000_000_000,
    };
    let html = dioxus::ssr::render_element(rsx! { StorageCard { storage } });
    assert!(html.contains("data-testid=\"admin-health-storage-card\""));
    assert!(html.contains("3.1 MB"));
    assert!(html.contains("512 B"));
    assert!(html.contains("Export-EPUB cache"));
    assert!(html.contains("2.0 KB"));
    assert!(html.contains("cap"));
}

#[test]
fn last_errors_card_renders_empty_state() {
    let html = dioxus::ssr::render_element(rsx! { LastErrorsCard { entries: Vec::new() } });
    assert!(html.contains("data-testid=\"admin-health-errors-empty\""));
}

#[test]
fn last_errors_card_renders_a_row_per_entry() {
    let entries = vec![CapturedError {
        timestamp: 1_700_000_000,
        target: "omnibus::test".into(),
        message: "boom".into(),
        book_uuid: None,
        file: None,
    }];
    let html = dioxus::ssr::render_element(rsx! { LastErrorsCard { entries } });
    assert!(html.contains("data-testid=\"admin-health-errors-table\""));
    assert!(html.contains("boom"));
    assert!(html.contains("2023-11-14 22:13:20 UTC"));
}
