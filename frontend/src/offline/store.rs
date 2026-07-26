//! SQLite-backed durable store for the offline layer: replica cache rows,
//! the mutation outbox, the download registry, and small metadata — one
//! `offline.db` under the app data dir. The connection is owned by a
//! dedicated worker thread fed closures over an `mpsc` channel (mirroring
//! `data::token_store`'s persistence worker) so callers never block on disk
//! I/O and writes can't race each other.

use std::path::PathBuf;
use std::sync::OnceLock;

use rusqlite::{params, Connection};
use tokio::sync::{mpsc, oneshot};

/// One cached JSON payload — the local replica of a server response.
#[derive(Debug, Clone, PartialEq)]
pub struct CacheRow {
    pub payload: String,
    pub fetched_at: i64,
}

/// One queued mutation awaiting drain, in enqueue order.
#[derive(Debug, Clone, PartialEq)]
pub struct OpRow {
    pub id: i64,
    pub kind: String,
    pub payload: String,
    pub attempts: i64,
}

/// Download registry row — one per (book uuid, format), so a dual-format
/// book holds two independent downloads (AC4 isolation).
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadRow {
    pub book_uuid: String,
    pub format: String,
    /// Display title snapshot for the account Downloads list (usable
    /// offline without a metadata lookup).
    pub title: String,
    pub file_id: Option<i64>,
    pub status: String,
    pub total_bytes: Option<i64>,
    pub downloaded_bytes: i64,
    /// JSON-encoded `Vec<downloads::PlannedFile>`.
    pub files: String,
    pub error: Option<String>,
    pub updated_at: i64,
}

/// Failure opening or preparing the offline SQLite store. Wraps the raw
/// `rusqlite::Error` so it never crosses this module's boundary directly
/// (see `.claude/rules/02-error-handling.md`'s boundary rule).
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Any `rusqlite` failure opening the connection, setting pragmas, or
    /// applying the schema.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// The OS refused to spawn the dedicated worker thread.
    #[error("could not spawn the offline store worker thread: {0}")]
    WorkerSpawn(#[source] std::io::Error),
}

type Job = Box<dyn FnOnce(&Connection) + Send>;

/// Handle to the store worker. Cheap to clone; the tokio unbounded sender is
/// `Sync`, so the `&'static Store` from [`store`] is shareable across the
/// UI, media-server, and download threads.
#[derive(Clone)]
pub struct Store {
    tx: mpsc::UnboundedSender<Job>,
}

static GLOBAL: OnceLock<Option<Store>> = OnceLock::new();

/// Open the process-wide store at `{data_dir}/offline.db`. Idempotent; a
/// failed open is cached so the app degrades to online-only for this launch.
pub fn init_global() {
    let _ = GLOBAL.get_or_init(|| {
        let dir = crate::data::app_dirs::data_dir()?;
        match Store::open(dir.join("offline.db")) {
            Ok(store) => Some(store),
            Err(e) => {
                tracing::warn!(error = %e, "could not open offline store; offline features disabled this launch");
                None
            }
        }
    });
}

/// The process-wide store, or `None` when unavailable (no writable data dir,
/// SQLite open failure, or `init_global` never ran — e.g. host unit tests).
pub fn store() -> Option<&'static Store> {
    GLOBAL.get().and_then(|s| s.as_ref())
}

/// Test-only global store against a leaked tempdir, so cache/outbox tests can
/// exercise the real read/write paths. First caller wins for the process
/// lifetime — tests must use distinct keys instead of assuming a fresh DB.
#[cfg(test)]
pub(crate) fn init_global_for_tests() {
    GLOBAL.get_or_init(|| {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("offline.db");
        // Leak the tempdir guard: the DB must outlive every test in the run.
        std::mem::forget(dir);
        Some(Store::open(path).expect("open test store"))
    });
}

/// Unix seconds now. Saturates at 0 rather than panicking on a pre-epoch
/// clock.
pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Store {
    /// Open (or create) the database at `path` and spawn the worker thread.
    /// The connection and schema are prepared on the caller's thread so open
    /// failures surface immediately instead of as silent no-op sends.
    pub fn open(path: PathBuf) -> Result<Store, StoreError> {
        let conn = Connection::open(&path)?;
        // WAL keeps readers (none today, but cheap insurance) from blocking
        // the worker's writes; NORMAL sync is durable enough for a cache +
        // replayable outbox and much kinder to mobile flash.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        apply_schema(&conn)?;

        let (tx, mut rx) = mpsc::unbounded_channel::<Job>();
        std::thread::Builder::new()
            .name("omnibus-offline-store".into())
            .spawn(move || {
                let conn = conn;
                while let Some(job) = rx.blocking_recv() {
                    job(&conn);
                }
            })
            .map_err(StoreError::WorkerSpawn)?;
        Ok(Store { tx })
    }

    /// Run `f` on the worker and await its result. `None` when the worker is
    /// gone (channel closed) — callers treat that as "store unavailable".
    pub(crate) async fn run<T, F>(&self, f: F) -> Option<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> T + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Box::new(move |conn| {
                let _ = tx.send(f(conn));
            }))
            .ok()?;
        rx.await.ok()
    }

    /// Run `f` on the worker without awaiting. Ordering with later `run`
    /// calls is preserved by the single channel.
    pub(crate) fn run_detached<F>(&self, f: F)
    where
        F: FnOnce(&Connection) + Send + 'static,
    {
        let _ = self.tx.send(Box::new(f));
    }

    /// Blocking variant of [`Store::run`] for pre-launch init paths (no
    /// async runtime exists yet when `offline::init` hydrates registries).
    /// Never call from async code. `None` on worker loss or timeout.
    pub(crate) fn run_blocking<T, F>(&self, f: F) -> Option<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> T + Send + 'static,
    {
        let (tx, rx) = std::sync::mpsc::channel();
        self.tx
            .send(Box::new(move |conn| {
                let _ = tx.send(f(conn));
            }))
            .ok()?;
        rx.recv_timeout(std::time::Duration::from_secs(5)).ok()
    }

    /// Blocking snapshot of every `(key, payload)` under a key prefix — the
    /// cold-start hydration read for the in-memory progress maps.
    pub(crate) fn kv_prefix_blocking(&self, prefix: &str) -> Vec<(String, String)> {
        let pattern = format!("{}%", prefix.replace('%', "\\%").replace('_', "\\_"));
        self.run_blocking(move |conn| {
            let mut stmt = match conn
                .prepare("SELECT key, payload FROM cache WHERE key LIKE ?1 ESCAPE '\\'")
            {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };
            let rows = stmt.query_map(params![pattern], |row| Ok((row.get(0)?, row.get(1)?)));
            match rows {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(_) => Vec::new(),
            }
        })
        .unwrap_or_default()
    }

    // --- cache ---

    /// Fetch one cached payload.
    pub async fn kv_get(&self, key: &str) -> Option<CacheRow> {
        let key = key.to_string();
        self.run(move |conn| {
            conn.query_row(
                "SELECT payload, fetched_at FROM cache WHERE key = ?1",
                params![key],
                |row| {
                    Ok(CacheRow {
                        payload: row.get(0)?,
                        fetched_at: row.get(1)?,
                    })
                },
            )
            .ok()
        })
        .await
        .flatten()
    }

    /// Upsert one cached payload, stamping `fetched_at = now`.
    pub fn kv_put(&self, key: &str, payload: String) {
        let key = key.to_string();
        self.run_detached(move |conn| {
            log_err(conn.execute(
                "INSERT INTO cache (key, payload, fetched_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET payload = ?2, fetched_at = ?3",
                params![key, payload, now_secs()],
            ));
        });
    }

    /// Delete one cached payload.
    pub fn kv_delete(&self, key: &str) {
        let key = key.to_string();
        self.run_detached(move |conn| {
            log_err(conn.execute("DELETE FROM cache WHERE key = ?1", params![key]));
        });
    }

    /// Every `(key, payload)` under a key prefix (async twin of
    /// [`Store::kv_prefix_blocking`]) — powers cache scans like "which
    /// cached highlight list contains id N".
    pub async fn kv_prefix(&self, prefix: &str) -> Vec<(String, String)> {
        let pattern = format!("{}%", prefix.replace('%', "\\%").replace('_', "\\_"));
        self.run(move |conn| {
            let mut stmt = match conn
                .prepare("SELECT key, payload FROM cache WHERE key LIKE ?1 ESCAPE '\\'")
            {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };
            let rows = stmt.query_map(params![pattern], |row| Ok((row.get(0)?, row.get(1)?)));
            match rows {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(_) => Vec::new(),
            }
        })
        .await
        .unwrap_or_default()
    }

    /// Delete every cache row whose key starts with `prefix`.
    pub fn kv_delete_prefix(&self, prefix: &str) {
        let pattern = format!("{}%", prefix.replace('%', "\\%").replace('_', "\\_"));
        self.run_detached(move |conn| {
            log_err(conn.execute(
                "DELETE FROM cache WHERE key LIKE ?1 ESCAPE '\\'",
                params![pattern],
            ));
        });
    }

    /// Rename a cache key in place (temp-id → server-id remap), replacing
    /// any row already at `to`.
    pub fn kv_rename(&self, from: &str, to: &str) {
        let (from, to) = (from.to_string(), to.to_string());
        self.run_detached(move |conn| {
            log_err(conn.execute("DELETE FROM cache WHERE key = ?1", params![to]));
            log_err(conn.execute(
                "UPDATE cache SET key = ?2 WHERE key = ?1",
                params![from, to],
            ));
        });
    }

    // --- outbox ---

    /// Append a mutation to the outbox. When `coalesce_key` is set, any
    /// queued op with the same key is replaced (local last-write-wins), and
    /// the fresh AUTOINCREMENT id keeps ordering relative to other kinds.
    pub fn ops_append(&self, kind: &str, payload: String, coalesce_key: Option<String>) {
        let kind = kind.to_string();
        self.run_detached(move |conn| {
            if let Some(ck) = &coalesce_key {
                log_err(conn.execute("DELETE FROM ops WHERE coalesce_key = ?1", params![ck]));
            }
            log_err(conn.execute(
                "INSERT INTO ops (kind, coalesce_key, payload, created_at, attempts)
                 VALUES (?1, ?2, ?3, ?4, 0)",
                params![kind, coalesce_key, payload, now_secs()],
            ));
        });
    }

    /// All queued ops in enqueue (id) order.
    pub async fn ops_list(&self) -> Vec<OpRow> {
        self.run(|conn| {
            let mut stmt =
                match conn.prepare("SELECT id, kind, payload, attempts FROM ops ORDER BY id ASC") {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(error = %e, "offline store query failed");
                        return Vec::new();
                    }
                };
            let rows = stmt.query_map([], |row| {
                Ok(OpRow {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    payload: row.get(2)?,
                    attempts: row.get(3)?,
                })
            });
            match rows {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(e) => {
                    tracing::warn!(error = %e, "offline store query failed");
                    Vec::new()
                }
            }
        })
        .await
        .unwrap_or_default()
    }

    /// Number of queued ops (the "pending changes" badge).
    pub async fn ops_count(&self) -> usize {
        self.run(|conn| {
            conn.query_row("SELECT COUNT(*) FROM ops", [], |row| row.get::<_, i64>(0))
                .unwrap_or(0) as usize
        })
        .await
        .unwrap_or(0)
    }

    /// Remove ops by id (drained, dropped, or cancelled).
    pub async fn ops_delete_many(&self, ids: Vec<i64>) {
        self.run(move |conn| {
            for id in ids {
                log_err(conn.execute("DELETE FROM ops WHERE id = ?1", params![id]));
            }
        })
        .await;
    }

    /// Record a failed drain attempt for backoff bookkeeping.
    pub fn ops_bump_attempts(&self, id: i64) {
        self.run_detached(move |conn| {
            log_err(conn.execute(
                "UPDATE ops SET attempts = attempts + 1 WHERE id = ?1",
                params![id],
            ));
        });
    }

    /// Structurally rewrite queued op payloads (temp-id → server-id remap).
    /// `f` returns `Some(new_payload)` for rows to update. Awaited so the
    /// drain loop can rely on the remap being visible to its next read.
    pub async fn ops_rewrite<F>(&self, f: F)
    where
        F: Fn(&str, &str) -> Option<String> + Send + 'static,
    {
        self.run(move |conn| {
            let rows: Vec<(i64, String, String)> = {
                let mut stmt = match conn.prepare("SELECT id, kind, payload FROM ops") {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(error = %e, "offline store query failed");
                        return;
                    }
                };
                let collected =
                    match stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))) {
                        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                        Err(e) => {
                            tracing::warn!(error = %e, "offline store query failed");
                            return;
                        }
                    };
                collected
            };
            for (id, kind, payload) in rows {
                if let Some(new_payload) = f(&kind, &payload) {
                    log_err(conn.execute(
                        "UPDATE ops SET payload = ?2 WHERE id = ?1",
                        params![id, new_payload],
                    ));
                }
            }
        })
        .await;
    }

    /// Next client temp id: −1, −2, … persisted across restarts so a temp id
    /// is never reused while ops referencing it may still be queued.
    pub async fn next_temp_id(&self) -> Option<i64> {
        self.run(|conn| {
            let current: i64 = conn
                .query_row(
                    "SELECT value FROM meta WHERE key = 'temp_id_counter'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let next = current - 1;
            log_err(conn.execute(
                "INSERT INTO meta (key, value) VALUES ('temp_id_counter', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = ?1",
                params![next.to_string()],
            ));
            next
        })
        .await
    }

    // --- meta ---

    /// Read a small metadata value (`last_sync_at`, `last_user`, …).
    pub async fn meta_get(&self, key: &str) -> Option<String> {
        let key = key.to_string();
        self.run(move |conn| {
            conn.query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .ok()
        })
        .await
        .flatten()
    }

    /// Upsert a small metadata value.
    pub fn meta_put(&self, key: &str, value: String) {
        let key = key.to_string();
        self.run_detached(move |conn| {
            log_err(conn.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = ?2",
                params![key, value],
            ));
        });
    }

    // --- downloads ---

    /// Upsert one download registry row. Awaited: the engine's state machine
    /// depends on transitions landing before the UI generation bump.
    pub async fn downloads_upsert(&self, row: DownloadRow) {
        self.run(move |conn| {
            log_err(conn.execute(
                "INSERT INTO downloads
                   (book_uuid, format, title, file_id, status, total_bytes, downloaded_bytes, files, error, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(book_uuid, format) DO UPDATE SET
                   title = ?3, file_id = ?4, status = ?5, total_bytes = ?6,
                   downloaded_bytes = ?7, files = ?8, error = ?9, updated_at = ?10",
                params![
                    row.book_uuid,
                    row.format,
                    row.title,
                    row.file_id,
                    row.status,
                    row.total_bytes,
                    row.downloaded_bytes,
                    row.files,
                    row.error,
                    row.updated_at,
                ],
            ));
        })
        .await;
    }

    /// Every download registry row.
    pub async fn downloads_all(&self) -> Vec<DownloadRow> {
        self.run(|conn| {
            let mut stmt = match conn.prepare(
                "SELECT book_uuid, format, title, file_id, status, total_bytes,
                        downloaded_bytes, files, error, updated_at
                 FROM downloads ORDER BY updated_at DESC",
            ) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "offline store query failed");
                    return Vec::new();
                }
            };
            let rows = stmt.query_map([], |row| {
                Ok(DownloadRow {
                    book_uuid: row.get(0)?,
                    format: row.get(1)?,
                    title: row.get(2)?,
                    file_id: row.get(3)?,
                    status: row.get(4)?,
                    total_bytes: row.get(5)?,
                    downloaded_bytes: row.get(6)?,
                    files: row.get(7)?,
                    error: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            });
            match rows {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(e) => {
                    tracing::warn!(error = %e, "offline store query failed");
                    Vec::new()
                }
            }
        })
        .await
        .unwrap_or_default()
    }

    /// Remove one download registry row.
    pub async fn downloads_delete(&self, book_uuid: &str, format: &str) {
        let (book_uuid, format) = (book_uuid.to_string(), format.to_string());
        self.run(move |conn| {
            log_err(conn.execute(
                "DELETE FROM downloads WHERE book_uuid = ?1 AND format = ?2",
                params![book_uuid, format],
            ));
        })
        .await;
    }
}

/// Log-and-continue for best-effort writes: the offline layer must never
/// take the app down over a cache write.
fn log_err(r: Result<usize, rusqlite::Error>) {
    if let Err(e) = r {
        tracing::warn!(error = %e, "offline store write failed");
    }
}

fn apply_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version >= 1 {
        return Ok(());
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS cache (
             key        TEXT PRIMARY KEY,
             payload    TEXT NOT NULL,
             fetched_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS ops (
             id           INTEGER PRIMARY KEY AUTOINCREMENT,
             kind         TEXT NOT NULL,
             coalesce_key TEXT,
             payload      TEXT NOT NULL,
             created_at   INTEGER NOT NULL,
             attempts     INTEGER NOT NULL DEFAULT 0
         );
         CREATE INDEX IF NOT EXISTS idx_ops_coalesce
             ON ops(coalesce_key) WHERE coalesce_key IS NOT NULL;
         CREATE TABLE IF NOT EXISTS downloads (
             book_uuid        TEXT NOT NULL,
             format           TEXT NOT NULL,
             title            TEXT NOT NULL DEFAULT '',
             file_id          INTEGER,
             status           TEXT NOT NULL,
             total_bytes      INTEGER,
             downloaded_bytes INTEGER NOT NULL DEFAULT 0,
             files            TEXT NOT NULL,
             error            TEXT,
             updated_at       INTEGER NOT NULL,
             PRIMARY KEY (book_uuid, format)
         );
         CREATE TABLE IF NOT EXISTS meta (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         PRAGMA user_version = 1;",
    )
}

#[cfg(test)]
mod tests;
