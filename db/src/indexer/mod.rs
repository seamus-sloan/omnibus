//! Background ebook indexing (server-only). Scans the configured library,
//! diffs the result against the `books` table, and applies only the
//! per-book changes the on-disk state demands. Reindex is driven by
//! [`crate::worker::Worker`] on startup (when stale) and on settings save;
//! scans run via `spawn_blocking` so the axum runtime stays responsive.

use std::path::Path;

use sqlx::SqlitePool;

use crate::{books, ebook, sync};

mod audiobooks;
mod backfill;
mod ebooks;
mod progress;

pub use audiobooks::{reindex_audiobooks, reindex_audiobooks_with_progress};
pub(crate) use backfill::{
    backfill_chapters, backfill_epub_structure, backfill_page_counts, backfill_thumbs,
    backfill_word_counts,
};
pub use ebooks::{reindex, reindex_with_progress};
pub(crate) use progress::{
    diff_tallies, display_item, report_parse_progress, report_sync_progress, root_display_name,
};
pub use progress::{ScanUpdate, PHASE_PARSING, PHASE_SYNCING, PHASE_WALKING};

/// Errors returned by the pure-DB indexer reads (`is_stale`). The
/// transparent `Db` variant honors the `02-error-handling` boundary rule
/// — no raw `sqlx::Error` across the module boundary — while keeping the
/// `?` propagation clean. `reindex` and `reindex_audiobooks` stay on
/// `anyhow::Result` because they span filesystem scans + parsing
/// (foreign-system failure space).
#[derive(Debug, thiserror::Error)]
pub enum IndexerError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl From<crate::settings::SettingsError> for IndexerError {
    fn from(e: crate::settings::SettingsError) -> Self {
        match e {
            crate::settings::SettingsError::Db(inner) => IndexerError::Db(inner),
            // `Validation`/`Json` are only produced by `set_hardcover_api_key`
            // and the F5.1 metadata-precedence writes, which no indexer path
            // calls — keep the arms exhaustive but do not widen
            // `IndexerError`'s surface for cases the caller graph can't reach.
            crate::settings::SettingsError::Validation(msg) => IndexerError::Db(
                sqlx::Error::Protocol(format!("unexpected settings validation error: {msg}")),
            ),
            crate::settings::SettingsError::Json(inner) => IndexerError::Db(sqlx::Error::Protocol(
                format!("unexpected settings JSON error: {inner}"),
            )),
        }
    }
}

/// Reindex if the last successful index is older than this. One hour is a
/// compromise between responsiveness to on-disk changes and avoiding
/// thrashing the disk for users who leave the app open all day.
pub const REFRESH_AFTER_SECS: i64 = 60 * 60;

/// Circuit-breaker threshold: abort the removal pass when a
/// single scan would flag more than this fraction of a previously-populated
/// library missing. A legitimate bulk delete this large is rare; a scan
/// that would erase a fifth of the library is far more likely a transient
/// mount/permission fault, so we halt and serve the existing index rather
/// than commit the wipe.
pub const MASS_MISSING_FRACTION: f64 = 0.20;

/// A scan that flags at most this many books missing is always allowed
/// through, regardless of [`MASS_MISSING_FRACTION`]. Without a floor, the
/// circuit-breaker would trip on ordinary small-library edits (deleting 1
/// of 3 books is 33%). Only libraries larger than this can trip the
/// percentage guard.
pub const MASS_MISSING_MIN_ABSOLUTE: usize = 10;

/// Warn threshold: below [`MASS_MISSING_FRACTION`] (the #819 abort guard)
/// but still large enough that a dropped mount or partial sync shouldn't
/// ghost books silently. Half the abort fraction — a scan in this band
/// still proceeds (unlike the abort guard) but its `Done` state carries a
/// dismissible warning (issue #1057).
pub const MASS_MISSING_WARN_FRACTION: f64 = 0.10;

/// Error raised when the removal pass would flag an implausible share of the
/// library missing. Surfaced (not swallowed) so the worker logs it and the
/// existing index is left intact — the underlying files are almost certainly
/// still on disk behind a transient fault.
#[derive(Debug, thiserror::Error)]
#[error(
    "reindex aborted: {removed} of {total} books ({percent:.0}%) would be flagged missing — \
     refusing to erode the library on a likely-transient scan fault (issue #819)"
)]
pub struct MassMissingError {
    pub removed: usize,
    pub total: usize,
    pub percent: f64,
}

/// Decide whether a scan's enumeration can be trusted to drive the removal
/// pass. Untrustworthy when the walk was `incomplete` (a subdir read
/// failed), or when the root read **totally empty** (`saw_any_file ==
/// false`) while the DB still holds file-backed books under it — the
/// boot-race / unmounted-NFS case where an empty read would otherwise mark
/// the whole library missing.
///
/// `saw_any_file` counts files of *any* extension, so a shared root that
/// legitimately holds files of another format but none of this library's
/// (e.g. EPUBs under an audiobook reindex) stays trustworthy — only a truly
/// empty directory trips the guard.
///
/// A genuinely empty library (`db_has_files == false`) is trustworthy: the
/// first-ever scan of an empty root, or a library the user really did clear,
/// must still be indexable.
fn enumeration_is_trustworthy(incomplete: bool, saw_any_file: bool, db_has_files: bool) -> bool {
    if incomplete {
        return false;
    }
    // A previously-populated root that now reads totally empty is the
    // transient fault, not a real mass-delete — distrust it. Equivalently:
    // trust when the walk saw a file, or when the DB has nothing to lose.
    saw_any_file || !db_has_files
}

/// Trip the circuit-breaker when `removed` exceeds both the absolute floor
/// and [`MASS_MISSING_FRACTION`] of the file-backed library. Returns the
/// typed error to surface; `Ok(())` when the removal is within bounds.
fn check_mass_missing(removed: usize, db_file_backed: usize) -> Result<(), MassMissingError> {
    if removed <= MASS_MISSING_MIN_ABSOLUTE || db_file_backed == 0 {
        return Ok(());
    }
    // Ratio only feeds a threshold comparison and a log line; library sizes
    // sit far below f64's 2^52 exact-integer range.
    #[allow(clippy::cast_precision_loss)]
    let fraction = removed as f64 / db_file_backed as f64;
    if fraction > MASS_MISSING_FRACTION {
        return Err(MassMissingError {
            removed,
            total: db_file_backed,
            percent: fraction * 100.0,
        });
    }
    Ok(())
}

/// `true` when a scan's ghost count clears [`MASS_MISSING_WARN_FRACTION`]:
/// more than [`MASS_MISSING_MIN_ABSOLUTE`] books *and* more than the warn
/// fraction of the file-backed library. Reuses the abort guard's absolute
/// floor so an ordinary small-library edit never warns either. Callers only
/// reach this after [`check_mass_missing`] has already passed, so the warn
/// band is implicitly capped above by [`MASS_MISSING_FRACTION`].
fn ghost_warning_threshold_exceeded(removed: usize, db_file_backed: usize) -> bool {
    if removed <= MASS_MISSING_MIN_ABSOLUTE || db_file_backed == 0 {
        return false;
    }
    // Same threshold-only ratio as `check_mass_missing` above.
    #[allow(clippy::cast_precision_loss)]
    let fraction = removed as f64 / db_file_backed as f64;
    fraction > MASS_MISSING_WARN_FRACTION
}

/// Per-scan tallies handed back to the worker so it can decide whether to
/// attach a [`omnibus_shared::GhostFilesWarning`] to the task's `Done`
/// state. `removed` is this scan's ghost count; `file_backed_total` is the
/// file-backed library size [`check_mass_missing`] measured it against.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReindexStats {
    pub removed: usize,
    pub file_backed_total: usize,
    /// Relocations this scan **recognized**. Reported separately from
    /// `removed` and deliberately absent from both the abort breaker and
    /// the ghost warning — a move is not a ghost. Counts what the diff
    /// detected, not what the writer applied; a relocation the writer
    /// declines is logged on its own.
    pub moved: usize,
}

impl ReindexStats {
    /// Project this scan's tallies into a wire-facing warning when the
    /// ghost count clears [`MASS_MISSING_WARN_FRACTION`]; `None` below the
    /// threshold, which is the ordinary silent-ghosting path (AC2).
    pub fn ghost_warning(&self) -> Option<omnibus_shared::GhostFilesWarning> {
        if !ghost_warning_threshold_exceeded(self.removed, self.file_backed_total) {
            return None;
        }
        Some(omnibus_shared::GhostFilesWarning {
            removed: u32::try_from(self.removed).unwrap_or(u32::MAX),
            total: u32::try_from(self.file_backed_total).unwrap_or(u32::MAX),
        })
    }
}

/// True when a refresh should be kicked off: no state at all, or state
/// older than [`REFRESH_AFTER_SECS`].
///
/// ## Clock-failure behavior
///
/// The freshness check is `now - last_indexed >= REFRESH_AFTER_SECS`. If
/// reading the wall clock fails (`SystemTime::now()` is before the UNIX
/// epoch — only reachable with a badly misconfigured system clock), the
/// `.unwrap_or(last)` fallback substitutes the stored `last_indexed`
/// timestamp for `now`. That makes `now - last == 0`, so the function
/// returns `Ok(false)` and the reindex is **silently skipped**.
///
/// This is a deliberate "serve stale rather than thrash the disk"
/// tradeoff: when the clock is unreadable we can't tell how old the index
/// is, so rather than re-scanning the library on every poll (a clock that
/// stays broken would otherwise trigger a reindex on every call) we keep
/// serving the existing index until the clock recovers. The decision is
/// factored into the pure [`is_stale_decision`] so the window boundaries
/// and this clock-failure fallback (`now == last`) are pinned by the
/// `is_stale_decision_*` tests below; change it only with intent.
pub async fn is_stale(pool: &SqlitePool, library_path: &str) -> Result<bool, IndexerError> {
    let Some(last) = crate::settings::last_indexed_at(pool, library_path).await? else {
        return Ok(true);
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        // Clock unreadable (or post-Y292B overflow): substitute `last` so
        // `now - last == 0` and we serve stale (see the doc comment above).
        .unwrap_or(last);
    Ok(is_stale_decision(last, now))
}

/// Pure freshness decision used by [`is_stale`]: stale when the index is at
/// least [`REFRESH_AFTER_SECS`] old. Factored out so the window boundaries
/// and the clock-failure fallback (which calls this with `now == last`) are
/// unit-testable without depending on a readable wall clock.
fn is_stale_decision(last: i64, now: i64) -> bool {
    now - last >= REFRESH_AFTER_SECS
}

/// Result of [`diff_library`]. Each bucket is what the writer should do
/// for the corresponding subset of files.
#[derive(Debug, Default)]
pub struct ReindexDiff {
    /// Already in the DB and the on-disk stat matches. Writer does
    /// nothing — `books.id` is preserved by definition.
    pub unchanged: Vec<String>,
    /// Not in the DB. Writer parses + inserts.
    pub new: Vec<ebook::ParseTarget>,
    /// In the DB but the stat differs. Writer parses + UPDATEs in place
    /// (`books.id` preserved).
    pub changed: Vec<ebook::ParseTarget>,
    /// A file that changed path but not content: its stat pair matched
    /// exactly one vanished row and one arrived file. Writer repoints path
    /// columns only — no parse, no insert, no delete.
    pub moved: Vec<sync::MovedFile>,
    /// A file-backed book whose file is gone (by `books.uuid`). The writer
    /// drops its `book_files` row + clears FTS, retaining the `books` row as
    /// a fileless book; cover files get cleaned up post-commit.
    pub removed: Vec<String>,
    /// In the DB and on disk, but the DB row carries the post-migration
    /// `(mtime_epoch=0, size_bytes=0)` sentinel. Writer fills in the stat
    /// values without re-parsing the OPF — the one-time upgrade fast path.
    pub backfill: Vec<(String, i64, i64)>,
}

/// Pure classifier — no I/O. Compares a Phase-A stat output against the
/// current DB state and routes each file into one of the six buckets.
/// `library_root` is the absolute path the scanner walked; we join it
/// with each `filename` to fill `ParseTarget.absolute` so Phase B can
/// open files directly without re-walking.
///
/// `enumeration_trustworthy` gates the **Removed** bucket only: when the
/// caller could not fully enumerate the tree (a subdir read failed, or a
/// once-populated root read empty) it passes `false`, and `diff_library`
/// leaves `removed` empty so a partial scan can never flag a book missing.
/// The New / Changed / Backfill buckets are unaffected — an incomplete
/// scan still safely indexes the files it *did* see. **Moved** rides on
/// Removed: with no removal pass there is nothing to match arrivals
/// against, so it stays empty too.
///
/// # Bucket semantics
///
/// - **Unchanged** — on disk, in DB, `(mtime_epoch, size_bytes)` matches.
///   No work. `books.id` preserved.
/// - **New** — on disk, not in DB. Full Phase-B parse + insert.
/// - **Changed** — on disk, in DB, stat differs. Full Phase-B parse, then
///   UPDATE in place (preserves `books.id`).
/// - **Moved** — a vanished row and an arrived file carrying the same
///   `(size_bytes, mtime_epoch)`. Writer repoints path columns; identity
///   and every soft-ref user-data row ride along untouched. See below.
/// - **Removed** — a file-backed book whose file is gone. Its `book_files`
///   row is dropped so the book becomes a **fileless book** (the `books`
///   row + its soft-ref user data are retained, not deleted); a returning
///   file re-attaches via Changed. An already-fileless book is left alone.
///   Only populated when `enumeration_trustworthy` is `true`.
/// - **Backfill** — in DB, in disk, DB has the migration default
///   `(mtime_epoch=0, size_bytes=0)`. Treated as the sentinel for "fs
///   metadata never observed" (post-migration), so the writer only
///   updates the stat columns; the OPF is not re-parsed. Without this,
///   the first reindex after the migration would treat every existing
///   row as Changed.
///
/// ## Moved, and what it deliberately can't see
///
/// The buckets are disjoint, so a file classified Moved never enters New
/// and the writer's title+author auto-attach never sees it. Phase B parses
/// only New + Changed, so a moved file is never parsed and the writer has
/// no `IndexedBook` to fall back on: a declined relocation is a **skip**,
/// not a fallback, which is why the emit conditions stay conservative.
/// The pair must be unique on both sides — ambiguity declines rather than
/// guessing — `(0, 0)`, the never-observed sentinel a failed stat and a
/// pre-backfill row both carry, never participates, and the match is scoped
/// to a single **format**: a relocation rewrites path columns only, so it
/// cannot carry a file whose extension changed (see [`match_moved_files`]).
///
/// This detector is a byte identity and the writer's `(title_norm,
/// author_norm)` auto-attach is a semantic one, so each covers the other's
/// blind spot. Byte identity survives missing author metadata and tells
/// two editions of one book apart; it does not survive a move that also
/// retagged the title, or an mtime-lossy copy (`cp` without `-p`). The
/// title+author path is the fallback for both of those, at the cost of a
/// full parse plus ghost / DELETE / re-INSERT / FTS churn — and it runs in
/// the writer, *after* the `check_mass_missing` breaker, which a large
/// reorganization would otherwise trip before ever reaching it.
pub fn diff_library(
    disk: &[ebook::StatEntry],
    db: &[books::IndexedRow],
    library_root: &Path,
    enumeration_trustworthy: bool,
) -> ReindexDiff {
    use std::collections::HashMap;

    // The diff matches on `scan_key` — the library-relative path (F2) — so a
    // scan-root repoint (which leaves relative paths unchanged) preserves
    // every `books.uuid`. Empty scan_keys are synthetic "unreadable subdir"
    // placeholders / pre-backfill rows; skip them on both sides.
    let disk_by_key: HashMap<&str, &ebook::StatEntry> = disk
        .iter()
        .filter(|e| !e.scan_key.is_empty())
        .map(|e| (e.scan_key.as_str(), e))
        .collect();
    let db_by_key: HashMap<&str, &books::IndexedRow> = db
        .iter()
        .filter(|r| !r.scan_key.is_empty())
        .map(|r| (r.scan_key.as_str(), r))
        .collect();

    let mut out = ReindexDiff::default();
    // New and Removed are held back until the move matcher has had its pass
    // over them: a match takes one entry out of each, and only the leftovers
    // become the buckets the writer sees.
    let arrivals = classify_arrivals(&disk_by_key, &db_by_key, library_root, &mut out);
    let departures = collect_departures(db, &disk_by_key, enumeration_trustworthy);
    split_moved(&departures, &arrivals, library_root, &mut out);
    sort_buckets(&mut out);
    out
}

/// Turn one disk entry into a Phase-B parse target. The absolute path is
/// pre-joined so Phase B opens files directly without re-walking.
fn parse_target(entry: &ebook::StatEntry, library_root: &Path) -> ebook::ParseTarget {
    ebook::ParseTarget {
        filename: entry.filename.clone(),
        absolute: library_root.join(&entry.filename),
        mtime_epoch: entry.mtime_epoch,
        size_bytes: entry.size_bytes,
    }
}

/// Classify every file on disk against the DB, filling the Unchanged /
/// Changed / Backfill buckets directly. Files with no DB row of their own
/// are **returned** rather than pushed into New — the move matcher gets
/// first refusal on them.
fn classify_arrivals<'a>(
    disk_by_key: &std::collections::HashMap<&'a str, &'a ebook::StatEntry>,
    db_by_key: &std::collections::HashMap<&str, &books::IndexedRow>,
    library_root: &Path,
    out: &mut ReindexDiff,
) -> Vec<&'a ebook::StatEntry> {
    let mut arrivals: Vec<&ebook::StatEntry> = Vec::new();
    for (scan_key, entry) in disk_by_key {
        match db_by_key.get(scan_key) {
            None => arrivals.push(entry),
            Some(row) if !row.has_file => {
                // A fileless book whose file is back on disk. Route through
                // Changed: the writer re-creates the `book_files` row while
                // preserving the existing `books.uuid` (auto-relink, F2).
                out.changed.push(parse_target(entry, library_root));
            }
            Some(row) => {
                let never_observed = row.mtime_epoch == 0 && row.size_bytes == 0;
                let matches =
                    row.mtime_epoch == entry.mtime_epoch && row.size_bytes == entry.size_bytes;
                if never_observed {
                    out.backfill
                        .push((row.uuid.clone(), entry.mtime_epoch, entry.size_bytes));
                } else if matches {
                    out.unchanged.push(row.uuid.clone());
                } else {
                    out.changed.push(parse_target(entry, library_root));
                }
            }
        }
    }
    arrivals
}

/// File-backed rows whose path is no longer on disk — Removed candidates,
/// held back for the move matcher the same way arrivals are.
///
/// The removal pass runs only on a fully-trusted enumeration. On a partial
/// scan (unreadable subdir, or a once-populated root now reading empty) this
/// returns nothing, so no book is flagged missing and no `merged_uuids` row
/// is eroded (issue #819). An already-fileless book is left untouched.
fn collect_departures<'a>(
    db: &'a [books::IndexedRow],
    disk_by_key: &std::collections::HashMap<&str, &ebook::StatEntry>,
    enumeration_trustworthy: bool,
) -> Vec<&'a books::IndexedRow> {
    if !enumeration_trustworthy {
        return Vec::new();
    }
    db.iter()
        .filter(|row| !row.scan_key.is_empty() && row.has_file)
        .filter(|row| !disk_by_key.contains_key(row.scan_key.as_str()))
        .collect()
}

/// Pair departures with arrivals carrying the same bytes — one relocation,
/// not a deletion plus an insertion — then drain whatever is left into the
/// Removed and New buckets.
fn split_moved(
    departures: &[&books::IndexedRow],
    arrivals: &[&ebook::StatEntry],
    library_root: &Path,
    out: &mut ReindexDiff,
) {
    let mut departure_moved = vec![false; departures.len()];
    let mut arrival_moved = vec![false; arrivals.len()];
    for (d, a) in match_moved_files(departures, arrivals) {
        departure_moved[d] = true;
        arrival_moved[a] = true;
        out.moved.push(sync::MovedFile {
            uuid: departures[d].uuid.clone(),
            filename: arrivals[a].filename.clone(),
        });
    }
    for (i, entry) in arrivals.iter().enumerate() {
        if !arrival_moved[i] {
            out.new.push(parse_target(entry, library_root));
        }
    }
    for (i, row) in departures.iter().enumerate() {
        if !departure_moved[i] {
            out.removed.push(row.uuid.clone());
        }
    }
}

/// Stable order keeps the writer's behavior predictable across runs
/// (matters for the cover-file post-commit step, and for tests). `moved`
/// sorts on its content, not on the candidate indices the matcher returned
/// — those index vecs built by iterating a `HashMap`.
fn sort_buckets(out: &mut ReindexDiff) {
    out.new.sort_by(|a, b| a.filename.cmp(&b.filename));
    out.changed.sort_by(|a, b| a.filename.cmp(&b.filename));
    out.moved
        .sort_by(|a, b| (&a.filename, &a.uuid).cmp(&(&b.filename, &b.uuid)));
    out.unchanged.sort();
    out.removed.sort();
    out.backfill.sort_by(|a, b| a.0.cmp(&b.0));
}

/// Pair vanished rows with arrived files on the `(size_bytes, mtime_epoch)`
/// pair both sides of the diff already carry, **within one format**.
/// Returns `(removed index, new index)` pairs; every index appears at most
/// once.
///
/// A pair only matches when it is unique on **both** sides. An ambiguous
/// group falls through to filename-stem equality, and only where the stem
/// is itself unique on both sides — a tiebreaker, never a requirement,
/// because requiring it would kill pure renames, which are the common real
/// case. `(0, 0)` is the never-observed sentinel: a failed stat and a
/// pre-backfill row both carry it, so without skipping it every unstattable
/// file would match every other one.
///
/// ## Why the format is part of the key
///
/// A relocation rewrites path columns and nothing else, so it cannot change
/// `book_files.format` — and `format` is not decoration: `books::book_file_path`
/// rebuilds the on-disk path as `library / path / filename + "." +
/// lower(format)`, so a row whose format no longer matches its extension
/// resolves to a file that isn't there. Retyping `Book.m4a` to `Book.m4b`
/// preserves the bytes, so it would otherwise match on the stat pair alone
/// (and `path_stem` drops the extension, so the tiebreaker would *help* it
/// pair) — leaving a book unreachable from every endpoint, and stuck that
/// way, since the next scan finds the new path with a matching stat and
/// classifies it Unchanged.
///
/// A format change genuinely needs a Phase-B re-parse — a `.cbz` and a
/// `.epub` parse differently, and format drives HLS/transcode and reader
/// routing — so declining here, and letting the file fall through to the
/// Removed and New buckets, is the correct answer rather than a missed
/// optimization. Note that scoping the diff to a format *set*
/// (`EBOOK_FORMATS` / `AUDIOBOOK_FORMATS`) is not the same as scoping to a
/// format: both sets hold more than one today.
fn match_moved_files(
    removed: &[&books::IndexedRow],
    new: &[&ebook::StatEntry],
) -> Vec<(usize, usize)> {
    use std::collections::HashMap;

    // `(size_bytes, mtime_epoch, format)`.
    type MatchKey = (i64, i64, String);

    let mut removed_by_pair: HashMap<MatchKey, Vec<usize>> = HashMap::new();
    for (i, row) in removed.iter().enumerate() {
        if row.size_bytes == 0 && row.mtime_epoch == 0 {
            continue;
        }
        removed_by_pair
            .entry((row.size_bytes, row.mtime_epoch, path_format(&row.scan_key)))
            .or_default()
            .push(i);
    }
    let mut new_by_pair: HashMap<MatchKey, Vec<usize>> = HashMap::new();
    for (j, entry) in new.iter().enumerate() {
        if entry.size_bytes == 0 && entry.mtime_epoch == 0 {
            continue;
        }
        new_by_pair
            .entry((
                entry.size_bytes,
                entry.mtime_epoch,
                path_format(&entry.scan_key),
            ))
            .or_default()
            .push(j);
    }

    let mut out: Vec<(usize, usize)> = Vec::new();
    for (pair, removed_group) in &removed_by_pair {
        let Some(new_group) = new_by_pair.get(pair) else {
            continue;
        };
        if let ([r], [n]) = (removed_group.as_slice(), new_group.as_slice()) {
            out.push((*r, *n));
        } else {
            push_stem_matches(removed, new, removed_group, new_group, &mut out);
        }
    }
    out
}

/// Break an ambiguous stat-pair group on filename-stem equality, emitting
/// only the stems that resolve to exactly one candidate on each side.
/// Candidates the stem can't separate stay in Removed + New, where the
/// writer's title+author path still gets its chance at them.
fn push_stem_matches(
    removed: &[&books::IndexedRow],
    new: &[&ebook::StatEntry],
    removed_group: &[usize],
    new_group: &[usize],
    out: &mut Vec<(usize, usize)>,
) {
    use std::collections::HashMap;

    let mut removed_by_stem: HashMap<&str, Vec<usize>> = HashMap::new();
    for &i in removed_group {
        removed_by_stem
            .entry(path_stem(&removed[i].scan_key))
            .or_default()
            .push(i);
    }
    let mut new_by_stem: HashMap<&str, Vec<usize>> = HashMap::new();
    for &j in new_group {
        new_by_stem
            .entry(path_stem(&new[j].scan_key))
            .or_default()
            .push(j);
    }
    for (stem, removed_stem_group) in &removed_by_stem {
        let Some(new_stem_group) = new_by_stem.get(stem) else {
            continue;
        };
        if let ([r], [n]) = (removed_stem_group.as_slice(), new_stem_group.as_slice()) {
            out.push((*r, *n));
        }
    }
}

/// The leaf stem of a scan key — `Author/Title.epub` and the audiobook
/// group `Author/Title` both yield `Title`. Borrowed from the input, so the
/// tiebreaker's maps allocate nothing per candidate.
fn path_stem(scan_key: &str) -> &str {
    Path::new(scan_key)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(scan_key)
}

/// The uppercase extension of a scan key, derived exactly as
/// `crate::helpers::split_filename` derives `book_files.format` — so the
/// match key above gates on the value that actually drives path
/// resolution, not on a lookalike. A directory-shaped audiobook group key
/// carries no extension and yields `UNKNOWN`, so multi-part groups compare
/// equal to each other and never to a single-file group.
fn path_format(scan_key: &str) -> String {
    Path::new(scan_key)
        .extension()
        .map(|s| s.to_string_lossy().to_ascii_uppercase())
        .unwrap_or_else(|| "UNKNOWN".to_string())
}

/// Best-effort GC of books whose files have been missing past the retention
/// window and that carry no user data. Logged, never surfaced as an error so a
/// GC failure can never abort a reindex — matches the best-effort cover/FTS
/// pattern. The sweep is global (not library-scoped), so running it after each
/// per-library reindex is cheap and idempotent.
async fn gc_missing_files_best_effort(pool: &SqlitePool) {
    match crate::missing_files::gc_books_missing_files(
        pool,
        crate::missing_files::MISSING_FILES_RETENTION_DAYS,
    )
    .await
    {
        Ok(purged) if purged > 0 => tracing::info!(purged, "reindex: GC'd books missing files"),
        Ok(_) => {}
        Err(e) => tracing::error!(error = %e, "reindex: missing-files GC failed"),
    }
}

#[cfg(test)]
mod tests;
