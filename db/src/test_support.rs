//! Shared test-only helpers for the `omnibus-db` crate, gated behind
//! `#[cfg(any(test, feature = "test-support"))]` so non-test consumers don't
//! pay the compile cost. The single source of truth for cross-cutting helpers —
//! temp-dir builders, in-memory pool seeders, env-var guards, and the
//! `IndexedBook` factories. Module-local helpers stay next to their callers.

#![cfg(any(test, feature = "test-support"))]

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use omnibus_shared::{Contributor, EbookMetadata};
use sqlx::SqlitePool;

use crate::ebook::IndexedBook;

// ---------------------------------------------------------------------------
// Filesystem helpers
// ---------------------------------------------------------------------------

/// Build a unique temp directory per test invocation. Rust runs unit
/// tests in parallel by default, so a fixed path under `temp_dir()`
/// would collide between tests (and between repeated runs).
pub fn make_test_dir(suffix: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let pid = std::process::id();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("omnibus_test_{suffix}_{pid}_{seq}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("should create test dir");
    dir
}

// ---------------------------------------------------------------------------
// In-memory EPUB fixtures
// ---------------------------------------------------------------------------

/// IEEE CRC32 over `bytes` — the checksum each stored ZIP entry needs.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Assemble an uncompressed (stored) ZIP from `(name, bytes)` entries.
/// Stored entries keep the writer trivial — no deflate, just headers,
/// CRCs, and a central directory — and `EpubDoc` reads them fine.
pub fn build_stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();

    for (name, data) in entries {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let name_len = name.len() as u16;
        let size = data.len() as u32;

        // Local file header + the (stored) file data.
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method: stored
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&0u16.to_le_bytes()); // mod date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes()); // compressed size
        out.extend_from_slice(&size.to_le_bytes()); // uncompressed size
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        // Matching central-directory record, accumulated for the tail.
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0u16.to_le_bytes()); // flags
        central.extend_from_slice(&0u16.to_le_bytes()); // method
        central.extend_from_slice(&0u16.to_le_bytes()); // mod time
        central.extend_from_slice(&0u16.to_le_bytes()); // mod date
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk start
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }

    let cd_offset = out.len() as u32;
    let cd_size = central.len() as u32;
    out.extend_from_slice(&central);

    // End-of-central-directory record.
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // this disk
    out.extend_from_slice(&0u16.to_le_bytes()); // cd start disk
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment len
    out
}

/// Minimal valid EPUB container around the given spine documents. Each
/// `(href, xhtml)` pair becomes one manifest item + spine itemref, in
/// order, so `href`s map to spine indices 0..n. The OCF-required stored
/// `mimetype` entry comes first.
pub fn build_test_epub(spine: &[(&str, &str)]) -> Vec<u8> {
    assemble_test_epub(spine, "", "", &[])
}

/// [`build_test_epub`] plus an EPUB3 `nav.xhtml` carrying
/// `properties="nav"`, whose `<nav epub:type="toc">` lists `toc` as
/// `(title, href)` anchors. The nav document sits at the OPF root, so
/// hrefs are OPF-root-relative as written.
pub fn build_test_epub_with_nav(spine: &[(&str, &str)], toc: &[(&str, &str)]) -> Vec<u8> {
    let items: String = toc
        .iter()
        .map(|(title, href)| format!(r#"<li><a href="{href}">{title}</a></li>"#))
        .collect();
    let nav = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>nav</title></head>
<body><nav epub:type="toc"><ol>{items}</ol></nav></body>
</html>"#
    );
    assemble_test_epub(
        spine,
        r#"<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>"#,
        "",
        &[("nav.xhtml", nav.into_bytes())],
    )
}

/// [`build_test_epub`] plus an EPUB2-style `toc.ncx` wired via
/// `<spine toc="ncx">`, listing `toc` as `(title, src)` navPoints — the
/// shape the `epub` crate parses into `doc.toc`.
pub fn build_test_epub_with_ncx(spine: &[(&str, &str)], toc: &[(&str, &str)]) -> Vec<u8> {
    let points: String = toc
        .iter()
        .enumerate()
        .map(|(i, (title, src))| {
            let n = i + 1;
            format!(
                r#"<navPoint id="np{n}" playOrder="{n}"><navLabel><text>{title}</text></navLabel><content src="{src}"/></navPoint>"#
            )
        })
        .collect();
    let ncx = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
<head><meta name="dtb:uid" content="test-epub"/></head>
<docTitle><text>Test</text></docTitle>
<navMap>{points}</navMap>
</ncx>"#
    );
    assemble_test_epub(
        spine,
        r#"<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>"#,
        r#" toc="ncx""#,
        &[("toc.ncx", ncx.into_bytes())],
    )
}

fn assemble_test_epub(
    spine: &[(&str, &str)],
    extra_manifest: &str,
    spine_attrs: &str,
    extra_entries: &[(&str, Vec<u8>)],
) -> Vec<u8> {
    let container: &[u8] = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;
    let mut manifest = String::new();
    let mut spine_xml = String::new();
    for (i, (href, _)) in spine.iter().enumerate() {
        manifest.push_str(&format!(
            r#"<item id="it{i}" href="{href}" media-type="application/xhtml+xml"/>"#
        ));
        spine_xml.push_str(&format!(r#"<itemref idref="it{i}"/>"#));
    }
    manifest.push_str(extra_manifest);
    let opf = format!(
        r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="pub-id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="pub-id">test-epub</dc:identifier>
    <dc:title>Test</dc:title>
  </metadata>
  <manifest>{manifest}</manifest>
  <spine{spine_attrs}>{spine_xml}</spine>
</package>"#
    );
    let mut entries: Vec<(&str, &[u8])> = vec![
        ("mimetype", b"application/epub+zip"),
        ("META-INF/container.xml", container),
    ];
    let opf_bytes = opf.as_bytes().to_vec();
    entries.push(("content.opf", &opf_bytes));
    for (href, xhtml) in spine {
        entries.push((href, xhtml.as_bytes()));
    }
    for (href, bytes) in extra_entries {
        entries.push((href, bytes.as_slice()));
    }
    build_stored_zip(&entries)
}

/// Kepub-variant assembler: identical container shape to [`build_test_epub`],
/// with the caller passing spine bodies already wrapped in
/// `<span class="koboSpan" id="kobo.N.M">` markup (this helper does not
/// reimplement kepubify's segmentation).
pub fn build_test_kepub(spine: &[(&str, &str)]) -> Vec<u8> {
    build_test_epub(spine)
}

// ---------------------------------------------------------------------------
// Generic env-var guard
// ---------------------------------------------------------------------------

/// Process-wide lock serializing all env-var mutations in tests. A single
/// lock is used so guards for different variables still serialize against
/// one another — env-var space is process-global.
pub static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// RAII guard that sets one or more env vars for the lifetime of the
/// test, then restores each previous value on drop. Acquires `ENV_LOCK`
/// so parallel `cargo test` runs can't race on shared variables.
///
/// Create with `EnvVarGuard::set(var, value)`, then chain `.also_set(var,
/// value)` for additional vars — all managed under the single `ENV_LOCK`
/// acquisition. Pass `None` for `value` to _remove_ a variable for the
/// test's duration (the previous value is still restored on drop).
///
/// Use the `_os` variants when the value is an `OsStr` (e.g. a
/// `tempfile::TempDir`'s path) to avoid `to_str().unwrap()` at call sites
/// and to round-trip non-UTF-8 pre-existing values correctly.
pub struct EnvVarGuard {
    /// `(var_name, previous_value)` pairs, restored in reverse on drop.
    /// Snapshots use `OsString` via `var_os` so a pre-existing non-UTF-8
    /// value is restored verbatim instead of being treated as "unset".
    vars: Vec<(&'static str, Option<OsString>)>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl EnvVarGuard {
    /// Acquire `ENV_LOCK`, save the current value of `var`, and set it to
    /// `value` (or remove it when `value` is `None`).
    pub fn set(var: &'static str, value: Option<&str>) -> Self {
        Self::set_os(var, value.map(OsStr::new))
    }

    /// `OsStr` variant of [`set`] — accepts paths (e.g.
    /// `tmp.path().as_os_str()`) without forcing the caller through
    /// `to_str().unwrap()`.
    pub fn set_os(var: &'static str, value: Option<&OsStr>) -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prev = std::env::var_os(var);
        // SAFETY: ENV_LOCK is held; no other thread in this process
        // mutates the environment concurrently.
        unsafe { Self::apply(var, value) }
        Self {
            vars: vec![(var, prev)],
            _lock: lock,
        }
    }

    /// Set an additional env var under the already-held `ENV_LOCK`. The
    /// previous value is restored (in LIFO order) when the guard drops.
    pub fn also_set(self, var: &'static str, value: Option<&str>) -> Self {
        self.also_set_os(var, value.map(OsStr::new))
    }

    /// `OsStr` variant of [`also_set`].
    pub fn also_set_os(mut self, var: &'static str, value: Option<&OsStr>) -> Self {
        let prev = std::env::var_os(var);
        // SAFETY: ENV_LOCK is still held via `self._lock`.
        unsafe { Self::apply(var, value) }
        self.vars.push((var, prev));
        self
    }

    /// Inner: apply one set/remove under an already-held lock.
    unsafe fn apply(var: &'static str, value: Option<&OsStr>) {
        match value {
            Some(v) => std::env::set_var(var, v),
            None => std::env::remove_var(var),
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: ENV_LOCK is still held via `_lock`; no other thread
        // can mutate the environment while we restore. Restore in reverse
        // insertion order so nested also_set chains unwind correctly.
        for (var, prev) in self.vars.drain(..).rev() {
            unsafe {
                match prev {
                    Some(v) => std::env::set_var(var, v),
                    None => std::env::remove_var(var),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Covers env guard
// ---------------------------------------------------------------------------

/// RAII guard that points `OMNIBUS_COVERS_DIR` at a unique temp dir for
/// the lifetime of the test, then restores the previous value (and
/// removes the temp dir) on drop. Holds the shared [`ENV_LOCK`] — the
/// same lock `EnvVarGuard` uses — so it serializes against *every* other
/// env-var mutation in the crate, not just other `CoversTempDir`s. A
/// prior split lock let a test setting `OMNIBUS_COVERS_DIR` via
/// `EnvVarGuard` run concurrently with one here and stomp the value.
pub struct CoversTempDir {
    pub path: PathBuf,
    prev: Option<String>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl CoversTempDir {
    /// Point `$OMNIBUS_COVERS_DIR` at a fresh `tag`-named temp dir, holding the
    /// process-wide env lock until the guard drops.
    pub fn new(tag: &str) -> Self {
        let guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pid = std::process::id();
        let seq = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("omnibus_covers_{tag}_{pid}_{seq}"));
        let _ = std::fs::remove_dir_all(&path);
        let prev = std::env::var("OMNIBUS_COVERS_DIR").ok();
        // SAFETY: ENV_LOCK is held; no other thread mutates the env.
        unsafe {
            std::env::set_var("OMNIBUS_COVERS_DIR", &path);
        }
        Self {
            path,
            prev,
            _guard: guard,
        }
    }
}

impl Drop for CoversTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
        // SAFETY: ENV_LOCK is still held via `_guard`.
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var("OMNIBUS_COVERS_DIR", v),
                None => std::env::remove_var("OMNIBUS_COVERS_DIR"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Minimal book row seeder (bypasses the indexer)
// ---------------------------------------------------------------------------

/// Seed `count` minimal `books` rows under `/lib` using a recursive CTE.
/// Bypasses `replace_books` / the indexer entirely — callers that only
/// need rows to exist (e.g. response-cap tests) avoid the multi-second
/// cost of running the full pipeline.
pub async fn seed_minimal_books(pool: &SqlitePool, count: i64) {
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(pool)
        .await
        .unwrap();
    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = '/lib'")
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        r"
        WITH RECURSIVE n(i) AS (
            SELECT 1
            UNION ALL
            SELECT i + 1 FROM n WHERE i < ?
        )
        INSERT INTO books (uuid, scan_key, library_id, path, title, sort)
        SELECT 'uuid-' || i, 'b' || i || '.epub', ?, '/lib/b' || i, 'Title ' || i,
               'Title ' || printf('%010d', i)
          FROM n
        ",
    )
    .bind(count)
    .bind(lib_id)
    .execute(pool)
    .await
    .unwrap();
    // Give every seeded book a `book_files` row so the fileless filter (F2) —
    // which hides fileless books from list/count reads — doesn't drop them.
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch)
         SELECT id, 'EPUB', 'b' || id, 1, 1 FROM books WHERE library_id = ?",
    )
    .bind(lib_id)
    .execute(pool)
    .await
    .unwrap();
}

// ---------------------------------------------------------------------------
// IndexedBook factories (formerly `sync::test_helpers`)
// ---------------------------------------------------------------------------

/// Shared `IndexedBook` builder used by db tests across modules.
pub fn indexed(
    filename: &str,
    title: Option<&str>,
    authors: &[&str],
    subjects: &[&str],
    series: Option<(&str, &str)>,
    cover: Option<(&str, &[u8])>,
) -> IndexedBook {
    IndexedBook {
        metadata: EbookMetadata {
            filename: filename.into(),
            title: title.map(Into::into),
            creators: authors
                .iter()
                .map(|a| Contributor {
                    name: (*a).into(),
                    ..Default::default()
                })
                .collect(),
            subjects: subjects.iter().map(|s| (*s).to_string()).collect(),
            series: series.map(|(n, _)| n.into()),
            series_index: series.map(|(_, i)| i.into()),
            ..Default::default()
        },
        cover: cover.map(|(m, b)| (m.into(), b.to_vec())),
        mtime_epoch: 0,
        size_bytes: 0,
        word_count: None,
    }
}

/// Build an `IndexedBook` matching `indexed(...)` but with the supplied
/// (mtime_epoch, size_bytes). Used to drive the New + Changed branches
/// of `sync_books` with realistic fs metadata.
pub fn indexed_with_stat(
    filename: &str,
    title: Option<&str>,
    mtime_epoch: i64,
    size_bytes: i64,
) -> IndexedBook {
    IndexedBook {
        metadata: EbookMetadata {
            filename: filename.into(),
            title: title.map(Into::into),
            ..Default::default()
        },
        cover: None,
        mtime_epoch,
        size_bytes,
        word_count: None,
    }
}

// ---------------------------------------------------------------------------
// IndexedAudiobook factories
// ---------------------------------------------------------------------------

/// Shared `IndexedAudiobook` builder for tests that need audiobook rows.
pub fn indexed_audiobook(
    group_path: &str,
    title: &str,
    creator: Option<&str>,
) -> crate::audiobook::IndexedAudiobook {
    crate::audiobook::IndexedAudiobook {
        scan_key: group_path.into(),
        group_path: group_path.into(),
        format: "M4B".into(),
        title: title.into(),
        creator_name: creator.map(Into::into),
        cover: None,
        accent: None,
        parts: vec![crate::audiobook::AudiobookPart {
            ordinal: 0,
            filename: format!("{group_path}/part1.m4b"),
            size_bytes: 1000,
            mtime_epoch: 100,
            duration_seconds: 3600.0,
        }],
        chapters: vec![],
        total_size_bytes: 1000,
        max_mtime_epoch: 100,
        description: Some("Audiobook · 1h 00m".into()),
        error: None,
    }
}

// ---------------------------------------------------------------------------
// Discovery fixture seeders (formerly `discovery::test_helpers`)
// ---------------------------------------------------------------------------

/// Seed a small multi-author, multi-series, multi-tag fixture for the
/// discovery query tests. Returns the pool and a `CoversTempDir` guard
/// the caller must keep alive for the lifetime of the test.
pub async fn seed_discovery_fixture() -> (SqlitePool, CoversTempDir) {
    let guard = CoversTempDir::new("discovery");
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    crate::sync::replace_books(
        &pool,
        "/lib",
        vec![
            // Two-author book in Saga #1 with tag "fiction"
            indexed(
                "saga1.epub",
                Some("Saga: Book One"),
                &["Ada Lovelace", "Grace Hopper"],
                &["fiction", "classic"],
                Some(("Saga", "1")),
                None,
            ),
            // Sequel in Saga #2, same primary author + new tag
            indexed(
                "saga2.epub",
                Some("Saga: Book Two"),
                &["Ada Lovelace"],
                &["fiction"],
                Some(("Saga", "2")),
                None,
            ),
            // Standalone by Ada — no series
            indexed(
                "standalone.epub",
                Some("Standalone"),
                &["Ada Lovelace"],
                &["essay"],
                None,
                None,
            ),
            // Different-author, different-series book
            indexed(
                "other.epub",
                Some("Other Story"),
                &["Niklaus Wirth"],
                &["nonfiction"],
                Some(("Pioneers", "1")),
                None,
            ),
        ],
    )
    .await
    .unwrap();
    (pool, guard)
}

/// Look up an `authors.id` by name. Panics on miss — test helper only.
pub async fn author_id_by_name(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT id FROM authors WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Look up a `series.id` by name. Panics on miss — test helper only.
pub async fn series_id_by_name(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT id FROM series WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Seed `count` minimal `books` rows under `/lib`, all linked to one
/// author ("Prolific") and one series ("Mega"), via recursive CTEs.
/// Bypasses `replace_books`/the indexer — the discovery read caps only
/// depend on link rows existing — keeping the test fast even past the
/// 1k cap. Returns `(author_id, series_id)`.
pub async fn seed_books_for_one_author_and_series(pool: &SqlitePool, count: i64) -> (i64, i64) {
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(pool)
        .await
        .unwrap();
    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = '/lib'")
        .fetch_one(pool)
        .await
        .unwrap();
    let author_id: i64 = sqlx::query_scalar(
        "INSERT INTO authors (name, sort) VALUES ('Prolific', 'Prolific') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let series_id: i64 =
        sqlx::query_scalar("INSERT INTO series (name, sort) VALUES ('Mega', 'Mega') RETURNING id")
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query(
        r"
        WITH RECURSIVE n(i) AS (
            SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?
        )
        INSERT INTO books (uuid, library_id, path, title, sort, series_index)
        SELECT 'uuid-' || i, ?, '/lib/b' || i, 'Title ' || i,
               'Title ' || printf('%010d', i), i
          FROM n
        ",
    )
    .bind(count)
    .bind(lib_id)
    .execute(pool)
    .await
    .unwrap();
    // Link every seeded book to the author and the series.
    sqlx::query(
        "INSERT INTO books_authors_link (book, author, position)
         SELECT id, ?, 0 FROM books",
    )
    .bind(author_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO books_series_link (book, series)
         SELECT id, ? FROM books",
    )
    .bind(series_id)
    .execute(pool)
    .await
    .unwrap();
    (author_id, series_id)
}

// ---------------------------------------------------------------------------
// Cross-format attach / merge fixture seeders
// ---------------------------------------------------------------------------

/// Index one ebook under `/ebooks` through the real `sync_books` write
/// path and return its **minted** `books.uuid` (F2 — identity is no longer
/// path-derived, so we read it back by `scan_key`). Shared by the attach and
/// merge test suites so the seeding shape can't drift between them.
pub async fn seed_synced_ebook(
    pool: &SqlitePool,
    filename: &str,
    title: &str,
    author: &str,
) -> String {
    crate::sync::sync_books(
        pool,
        "/ebooks",
        crate::sync::SyncPlan {
            new_books: vec![indexed(filename, Some(title), &[author], &[], None, None)],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    uuid_by_scan_key(pool, &crate::helpers::scan_key_for(filename)).await
}

/// Index one audiobook under `/audio` through the real `sync_audiobooks`
/// write path and return its **minted** `books.uuid` (read back by scan_key).
pub async fn seed_synced_audiobook(
    pool: &SqlitePool,
    group_path: &str,
    title: &str,
    author: &str,
) -> String {
    let ab = indexed_audiobook(group_path, title, Some(author));
    crate::sync::sync_audiobooks(
        pool,
        "/audio",
        crate::sync::AudiobookSyncPlan {
            new_books: vec![ab],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    uuid_by_scan_key(pool, &crate::helpers::scan_key_for(group_path)).await
}

/// Read back the seeded book's identifier by its `scan_key` (the relative
/// path). For a native book that's the durable `books.uuid`; for a file that
/// auto-attached to an existing book (no own `books` row) it's the
/// `merged_uuids` ledger key — matching what the pre-F2 seeders returned, so
/// existing attach/merge assertions keep resolving through the same value.
pub async fn uuid_by_scan_key(pool: &SqlitePool, scan_key: &str) -> String {
    if let Some(uuid) = sqlx::query_scalar::<_, String>("SELECT uuid FROM books WHERE scan_key = ?")
        .bind(scan_key)
        .fetch_optional(pool)
        .await
        .unwrap()
    {
        return uuid;
    }
    sqlx::query_scalar::<_, String>("SELECT uuid FROM merged_uuids WHERE scan_key = ?")
        .bind(scan_key)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// One-scalar COUNT helper for table-shape assertions.
pub async fn count_rows(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

/// Seed one EPUB book whose `book_file_path` resolves to a real file at
/// `lib/sub/book.epub`, so exporters/converters that write next to the EPUB
/// land inside `lib`. Returns `(books.id, books.uuid)`. Raw SQL (not the
/// `sync` path) so the caller controls the library root — `seed_synced_ebook`
/// hard-codes `/ebooks` and can't point at a tempdir.
pub async fn seed_epub_book_at(pool: &SqlitePool, lib: &Path) -> (i64, String) {
    let lib_str = lib.to_string_lossy().to_string();
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(&lib_str)
        .execute(pool)
        .await
        .unwrap();
    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = ?")
        .bind(&lib_str)
        .fetch_one(pool)
        .await
        .unwrap();
    // Old `last_modified` so a freshly-written cache always reads as fresh.
    sqlx::query(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, last_modified)
         VALUES ('uuid-1', 'sub/book.epub', ?, 'sub', 'Title', '2000-01-01 00:00:00')",
    )
    .bind(lib_id)
    .execute(pool)
    .await
    .unwrap();
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = 'uuid-1'")
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch)
         VALUES (?, 'EPUB', 'book', 4, 1)",
    )
    .bind(book_id)
    .execute(pool)
    .await
    .unwrap();
    std::fs::create_dir_all(lib.join("sub")).unwrap();
    std::fs::write(lib.join("sub").join("book.epub"), b"epub").unwrap();
    (book_id, "uuid-1".to_string())
}

// ---------------------------------------------------------------------------
// Error ring buffer
// ---------------------------------------------------------------------------

/// Clear the process-wide error ring buffer (`crate::error_ring`). Exposed
/// here (not just `#[cfg(test)]` inside that module) so the tracing `Layer`
/// tests in `server/`, which depend on this crate via the `test-support`
/// feature, can reset shared state between cases.
pub fn clear_error_ring() {
    crate::error_ring::clear();
}
