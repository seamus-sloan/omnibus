//! Unit tests for the log reader: directory resolution, record parsing, and
//! the filter / newest-first pagination behaviour of `read_logs`.

use super::*;
use crate::test_support::make_test_dir;

/// Write one daily log file with the given raw JSON lines.
fn write_log(dir: &Path, date: &str, lines: &[&str]) {
    let path = dir.join(format!("omnibus.log.{date}"));
    std::fs::write(path, format!("{}\n", lines.join("\n"))).unwrap();
}

/// A minimal subscriber-shaped JSON record.
fn rec(ts: &str, level: &str, target: &str, message: &str) -> String {
    serde_json::json!({
        "timestamp": ts,
        "level": level,
        "target": target,
        "fields": { "message": message },
    })
    .to_string()
}

#[test]
fn resolve_log_dir_prefers_explicit_log_dir() {
    let dir = resolve_log_dir(Some("/custom/logs".into()), Some("/data".into()));
    assert_eq!(dir, PathBuf::from("/custom/logs"));
}

#[test]
fn resolve_log_dir_falls_back_to_data_dir_logs() {
    let dir = resolve_log_dir(None, Some("/srv/data".into()));
    assert_eq!(dir, PathBuf::from("/srv/data/logs"));
}

#[test]
fn resolve_log_dir_defaults_when_unset() {
    let dir = resolve_log_dir(None, None);
    assert_eq!(dir, PathBuf::from("./data/logs"));
}

#[test]
fn parse_record_extracts_core_fields_and_folds_extras() {
    let line = serde_json::json!({
        "timestamp": "2026-07-19T00:00:25.011938Z",
        "level": "INFO",
        "target": "omnibus::backend::covers",
        "fields": { "message": "hi", "status": 200 },
        "span": { "method": "GET", "path": "/x", "name": "request" },
    })
    .to_string();
    let r = parse_record(&line).unwrap();
    assert_eq!(r.level, "INFO");
    assert_eq!(r.target, "omnibus::backend::covers");
    assert_eq!(r.message, "hi");
    let fields = r.fields.unwrap();
    assert!(fields.contains("\"status\":200"));
    assert!(fields.contains("\"span\""));
    // The message is lifted out, never duplicated into the detail blob.
    assert!(!fields.contains("\"message\""));
}

#[test]
fn parse_record_folds_the_request_log_span_and_response_fields_into_extra() {
    // The shape `server::request_log` emits: client_ip / range on the span,
    // content_length / content_range on the headers-out event, and a second
    // end-of-body event carrying stream_duration_ms. None are core columns,
    // so all must land in the detail blob for the admin viewer to render.
    let done = serde_json::json!({
        "timestamp": "2026-09-04T03:19:14.000000Z",
        "level": "INFO",
        "target": "omnibus::request_log",
        "fields": {
            "message": "finished processing request",
            "latency": "2 ms",
            "status": 206,
            "content_length": "1048576",
            "content_range": "bytes 0-1048575/734003200",
        },
        "span": {
            "method": "GET",
            "path": "/api/audiobooks/uuid/parts/0",
            "client_ip": "203.0.113.9",
            "range": "bytes=0-1048575",
            "user_agent": "omnibus/90",
            "name": "request",
        },
    })
    .to_string();
    let r = parse_record(&done).unwrap();
    assert_eq!(r.message, "finished processing request");
    let fields: serde_json::Value = serde_json::from_str(&r.fields.unwrap()).unwrap();
    assert_eq!(fields["content_length"], "1048576");
    assert_eq!(fields["content_range"], "bytes 0-1048575/734003200");
    assert_eq!(fields["span"]["client_ip"], "203.0.113.9");
    assert_eq!(fields["span"]["range"], "bytes=0-1048575");

    let eos = serde_json::json!({
        "timestamp": "2026-09-04T03:19:16.000000Z",
        "level": "INFO",
        "target": "omnibus::request_log",
        "fields": { "message": "response body finished", "stream_duration_ms": 7200123 },
        "span": { "method": "GET", "path": "/api/audiobooks/uuid/parts/0", "name": "request" },
    })
    .to_string();
    let r = parse_record(&eos).unwrap();
    assert_eq!(r.message, "response body finished");
    let fields: serde_json::Value = serde_json::from_str(&r.fields.unwrap()).unwrap();
    assert_eq!(fields["stream_duration_ms"], 7200123);
    assert_eq!(fields["span"]["path"], "/api/audiobooks/uuid/parts/0");
}

#[test]
fn parse_record_rejects_blank_and_malformed_lines() {
    assert!(parse_record("").is_none());
    assert!(parse_record("{not json").is_none());
    // Missing the required level.
    assert!(parse_record(r#"{"timestamp":"2026-07-19T00:00:00Z"}"#).is_none());
}

#[tokio::test]
async fn read_logs_returns_empty_page_for_missing_dir() {
    let page = read_logs(Path::new("/no/such/omnibus/logs"), &LogQuery::default())
        .await
        .unwrap();
    assert!(page.records.is_empty());
    assert!(!page.has_more);
}

#[tokio::test]
async fn read_logs_orders_newest_first_across_files() {
    let dir = make_test_dir("logs_order");
    write_log(
        &dir,
        "2026-07-18",
        &[
            &rec("2026-07-18T09:00:00Z", "INFO", "a", "old-1"),
            &rec("2026-07-18T10:00:00Z", "INFO", "a", "old-2"),
        ],
    );
    write_log(
        &dir,
        "2026-07-19",
        &[
            &rec("2026-07-19T08:00:00Z", "INFO", "a", "new-1"),
            &rec("2026-07-19T09:00:00Z", "INFO", "a", "new-2"),
        ],
    );

    let page = read_logs(&dir, &LogQuery::default()).await.unwrap();
    let messages: Vec<_> = page.records.iter().map(|r| r.message.as_str()).collect();
    assert_eq!(messages, vec!["new-2", "new-1", "old-2", "old-1"]);
    assert!(!page.has_more);
}

#[tokio::test]
async fn read_logs_filters_by_level() {
    let dir = make_test_dir("logs_level");
    write_log(
        &dir,
        "2026-07-19",
        &[
            &rec("2026-07-19T08:00:00Z", "INFO", "a", "info"),
            &rec("2026-07-19T09:00:00Z", "ERROR", "a", "boom"),
            &rec("2026-07-19T10:00:00Z", "WARN", "a", "careful"),
        ],
    );

    let query = LogQuery {
        // Case-insensitive match.
        level: Some("error".into()),
        ..Default::default()
    };
    let page = read_logs(&dir, &query).await.unwrap();
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.records[0].message, "boom");
}

#[tokio::test]
async fn read_logs_filters_by_module_substring() {
    let dir = make_test_dir("logs_module");
    write_log(
        &dir,
        "2026-07-19",
        &[
            &rec(
                "2026-07-19T08:00:00Z",
                "INFO",
                "omnibus::backend::covers",
                "c",
            ),
            &rec(
                "2026-07-19T09:00:00Z",
                "INFO",
                "omnibus_db::worker::queue",
                "q",
            ),
        ],
    );

    let query = LogQuery {
        module: Some("BACKEND".into()),
        ..Default::default()
    };
    let page = read_logs(&dir, &query).await.unwrap();
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.records[0].message, "c");
}

#[tokio::test]
async fn read_logs_filters_by_time_range_inclusive_of_minute() {
    let dir = make_test_dir("logs_time");
    write_log(
        &dir,
        "2026-07-19",
        &[
            &rec("2026-07-19T07:59:59Z", "INFO", "a", "before"),
            &rec("2026-07-19T08:30:00Z", "INFO", "a", "inside"),
            &rec("2026-07-19T09:59:30Z", "INFO", "a", "edge"),
            &rec("2026-07-19T10:00:01Z", "INFO", "a", "after"),
        ],
    );

    let query = LogQuery {
        from: Some("2026-07-19T08:00".into()),
        // A to-the-minute bound still includes 09:59:30 (starts_with).
        to: Some("2026-07-19T09:59".into()),
        ..Default::default()
    };
    let page = read_logs(&dir, &query).await.unwrap();
    let messages: Vec<_> = page.records.iter().map(|r| r.message.as_str()).collect();
    assert_eq!(messages, vec!["edge", "inside"]);
}

#[tokio::test]
async fn read_logs_paginates_with_has_more() {
    let dir = make_test_dir("logs_page");
    let lines: Vec<String> = (0..5)
        .map(|i| {
            rec(
                &format!("2026-07-19T08:00:0{i}Z"),
                "INFO",
                "a",
                &format!("m{i}"),
            )
        })
        .collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    write_log(&dir, "2026-07-19", &refs);

    let first = read_logs(
        &dir,
        &LogQuery {
            per_page: 2,
            page: 0,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        first
            .records
            .iter()
            .map(|r| r.message.as_str())
            .collect::<Vec<_>>(),
        vec!["m4", "m3"]
    );
    assert!(first.has_more);

    let last = read_logs(
        &dir,
        &LogQuery {
            per_page: 2,
            page: 2,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        last.records
            .iter()
            .map(|r| r.message.as_str())
            .collect::<Vec<_>>(),
        vec!["m0"]
    );
    assert!(!last.has_more);
}

#[tokio::test]
async fn read_logs_ignores_non_log_files() {
    let dir = make_test_dir("logs_stray");
    write_log(
        &dir,
        "2026-07-19",
        &[&rec("2026-07-19T08:00:00Z", "INFO", "a", "real")],
    );
    std::fs::write(dir.join("notes.txt"), "not a log line\n").unwrap();

    let page = read_logs(&dir, &LogQuery::default()).await.unwrap();
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.records[0].message, "real");
}
