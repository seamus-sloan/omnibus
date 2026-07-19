//! Reader over the on-disk rolling JSON logs for the admin log viewer.
//!
//! Parses the `tracing-subscriber` JSON records the server writes, applies the
//! level/module/time-range filters, and returns one newest-first page. The
//! writer lives in `server::logging`; both resolve the directory through
//! [`log_dir`] so reader and writer always agree.

use std::path::{Path, PathBuf};

use omnibus_shared::logs::{LogPage, LogQuery, LogRecord};

#[cfg(test)]
mod tests;

/// Directory holding the on-disk JSON logs. `$OMNIBUS_LOG_DIR` is used verbatim
/// when set; otherwise `<$OMNIBUS_DATA_DIR>/logs` (data dir default `./data`),
/// mirroring the other durable-storage dirs. Kept here (not in `server`) so the
/// frontend RPC — which can't depend on the `server` crate — reaches the same
/// path the writer uses.
pub fn log_dir() -> PathBuf {
    resolve_log_dir(
        std::env::var("OMNIBUS_LOG_DIR").ok(),
        std::env::var("OMNIBUS_DATA_DIR").ok(),
    )
}

/// Pure resolution of [`log_dir`] from its two env inputs, split out so the
/// precedence is testable without mutating process env.
fn resolve_log_dir(log_dir: Option<String>, data_dir: Option<String>) -> PathBuf {
    if let Some(dir) = log_dir {
        return PathBuf::from(dir);
    }
    let base = data_dir.unwrap_or_else(|| "./data".into());
    PathBuf::from(base).join("logs")
}

/// Read one filtered, newest-first page of log records from `dir`.
///
/// The daily-rolling files are named `omnibus.log.YYYY-MM-DD`, so a descending
/// filename sort is a descending date sort; within a file the lines are
/// chronological, so reversing them yields newest-first. Records accumulate in
/// that global newest-first order and reading stops as soon as the requested
/// page (plus one lookahead record for `has_more`) is satisfied, bounding the
/// work to roughly `offset + per_page` matches rather than the whole corpus. A
/// missing directory is an empty page, not an error — logging may be disabled.
pub async fn read_logs(dir: &Path, query: &LogQuery) -> anyhow::Result<LogPage> {
    let per_page = query.effective_per_page() as usize;
    let offset = query.page as usize * per_page;
    // One extra beyond the page tells us whether a further match exists.
    let want = offset + per_page + 1;

    let files = log_files_newest_first(dir).await?;
    let mut matched: Vec<LogRecord> = Vec::new();
    'files: for file in files {
        let content = match tokio::fs::read_to_string(&file).await {
            Ok(c) => c,
            // A file that vanished or turned unreadable between listing and
            // read (rotation, permissions) shouldn't sink the whole query.
            Err(_) => continue,
        };
        for line in content.lines().rev() {
            if line.is_empty() {
                continue;
            }
            if let Some(record) = parse_record(line) {
                if matches(&record, query) {
                    matched.push(record);
                    if matched.len() >= want {
                        break 'files;
                    }
                }
            }
        }
    }

    let has_more = matched.len() > offset + per_page;
    let records = matched.into_iter().skip(offset).take(per_page).collect();
    Ok(LogPage {
        records,
        page: query.page,
        per_page: query.effective_per_page(),
        has_more,
    })
}

/// List the log files in `dir` sorted newest-first by name. Returns an empty
/// list (not an error) when the directory is absent.
async fn log_files_newest_first(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut files = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        // The subscriber only ever writes `omnibus.log.<date>` here; guard on
        // the prefix so a stray file can't be parsed as log lines.
        let is_log = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("omnibus.log"));
        if is_log {
            files.push(path);
        }
    }
    files.sort_unstable();
    files.reverse();
    Ok(files)
}

/// Parse one JSON log line into a [`LogRecord`]. Returns `None` for a blank or
/// malformed line, or one missing the required timestamp/level, so a partially
/// written trailing line never aborts a read.
fn parse_record(line: &str) -> Option<LogRecord> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let timestamp = value.get("timestamp")?.as_str()?.to_string();
    let level = value.get("level")?.as_str()?.to_string();
    let target = value
        .get("target")
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .to_string();

    let (message, fields) = split_fields(value.get("fields"), value.get("span"));
    Some(LogRecord {
        timestamp,
        level,
        target,
        message,
        fields,
    })
}

/// Pull the `message` out of the event's `fields` object and fold everything
/// else — the remaining fields plus the request `span` (method/path) — into a
/// compact JSON detail string. Returns `("", None)` when there's nothing but a
/// message (or not even that).
fn split_fields(
    fields: Option<&serde_json::Value>,
    span: Option<&serde_json::Value>,
) -> (String, Option<String>) {
    let mut extra = serde_json::Map::new();
    let mut message = String::new();

    if let Some(serde_json::Value::Object(map)) = fields {
        for (key, val) in map {
            if key == "message" {
                message = val.as_str().unwrap_or_default().to_string();
            } else {
                extra.insert(key.clone(), val.clone());
            }
        }
    }
    if let Some(span) = span {
        extra.insert("span".to_string(), span.clone());
    }

    let detail = if extra.is_empty() {
        None
    } else {
        serde_json::to_string(&serde_json::Value::Object(extra)).ok()
    };
    (message, detail)
}

/// Whether a record satisfies every non-empty filter in `query`. Level is an
/// exact case-insensitive match; module is a case-insensitive substring of the
/// target; from/to are lexicographic bounds on the (fixed-width) timestamp,
/// both inclusive.
fn matches(record: &LogRecord, query: &LogQuery) -> bool {
    if let Some(level) = non_empty(&query.level) {
        if !record.level.eq_ignore_ascii_case(level) {
            return false;
        }
    }
    if let Some(module) = non_empty(&query.module) {
        if !record
            .target
            .to_lowercase()
            .contains(&module.to_lowercase())
        {
            return false;
        }
    }
    if let Some(from) = non_empty(&query.from) {
        if record.timestamp.as_str() < from {
            return false;
        }
    }
    if let Some(to) = non_empty(&query.to) {
        // `to` is an ISO-8601 prefix; a record inside that minute sorts after
        // the bare prefix but should still match, hence the `starts_with`.
        if record.timestamp.as_str() > to && !record.timestamp.starts_with(to) {
            return false;
        }
    }
    true
}

/// A filter value counts only when present and non-blank.
fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|s| !s.is_empty())
}
