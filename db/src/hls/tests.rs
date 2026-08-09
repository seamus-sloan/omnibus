//! Unit tests for the HLS transcode cache.
//!
//! Covers the pure helpers (`build_manifest`, `parse_ffmpeg_progress_us`,
//! `ffmpeg_progress_fraction`) plus the on-disk-sentinel + orphan-recovery
//! invariants that the status handler depends on.

use std::time::{Duration, SystemTime};

use super::*;
use crate::test_support::EnvVarGuard;

/// Create a unique tempdir and point `OMNIBUS_DATA_DIR` at it for the
/// test's duration. Returns both the guard (holds `ENV_LOCK` + restores
/// the env var on drop) and the `TempDir` handle (keeps the directory
/// alive until the end of the test).
fn data_dir_guard() -> (EnvVarGuard, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let guard = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(dir.path().as_os_str()));
    (guard, dir)
}

/// Backdate the mtime of `path` by `age`. Uses `File::set_modified` so
/// we don't take on a `filetime` dev-dep. Panics on failure — test
/// helper only, and the temp dir is always writable.
fn backdate(path: &std::path::Path, age: Duration) {
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open progress sentinel");
    let when = SystemTime::now() - age;
    f.set_modified(when).expect("set mtime");
}

#[test]
fn build_manifest_with_zero_duration_emits_stub() {
    let m = build_manifest(&[HlsPart {
        ordinal: 0,
        filename: "track.mp3".into(),
        duration_seconds: 0.0,
    }]);
    assert!(m.contains("#EXTM3U"));
    assert!(m.contains("seg-0000.ts"));
    assert!(m.contains("#EXT-X-ENDLIST"));
}

#[test]
fn build_manifest_calculates_correct_segment_count() {
    // 25 seconds → 3 segments (10, 10, 5).
    let parts = vec![
        HlsPart {
            ordinal: 0,
            filename: "p1.mp3".into(),
            duration_seconds: 15.0,
        },
        HlsPart {
            ordinal: 1,
            filename: "p2.mp3".into(),
            duration_seconds: 10.0,
        },
    ];
    let m = build_manifest(&parts);
    let extinf_count = m.lines().filter(|l| l.starts_with("#EXTINF:")).count();
    assert_eq!(extinf_count, 3, "expected 3 segments for 25 s total");
    // Last segment duration = 25 - 20 = 5.
    assert!(
        m.contains("#EXTINF:5.000,"),
        "last segment should be 5.000 s"
    );
}

#[test]
fn read_progress_returns_zero_when_sentinel_absent() {
    // Use a book_id that will never exist on the test filesystem.
    assert_eq!(read_progress(i64::MAX, AUDIO64), 0.0);
}

#[test]
fn is_ready_returns_false_when_sentinel_absent() {
    assert!(!is_ready(i64::MAX, AUDIO64));
}

#[test]
fn has_failed_returns_false_when_marker_absent() {
    assert!(!has_failed(i64::MAX, AUDIO64));
}

#[test]
fn failed_path_lives_at_book_id_level_not_inside_segment_dir() {
    // Invariant the `.failed` marker relies on:
    // `cleanup_segment_dir` wipes the segment dir, then writes the
    // marker one level up so the flag survives. If this path layout
    // ever drifts the cleanup will silently lose the marker.
    let p = failed_path(42, AUDIO64);
    assert_eq!(
        p.file_name().and_then(|s| s.to_str()),
        Some("audio64.failed"),
    );
    assert_eq!(
        p.parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str()),
        Some("42"),
        "marker must live at <hls_dir>/<book_id>/, not inside the segment dir",
    );
}

#[test]
fn ffmpeg_manifest_path_lives_inside_segment_dir() {
    let p = ffmpeg_manifest_path(42, AUDIO64);
    assert_eq!(p.file_name().and_then(|s| s.to_str()), Some("index.m3u8"));
    assert_eq!(
        p.parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str()),
        Some(AUDIO64),
    );
}

#[test]
fn read_ffmpeg_manifest_returns_none_when_file_absent() {
    assert_eq!(read_ffmpeg_manifest(i64::MAX, AUDIO64), None);
}

#[test]
fn progress_path_lives_inside_segment_dir() {
    let p = progress_path(42, AUDIO64);
    assert_eq!(p.file_name().and_then(|s| s.to_str()), Some(".progress"));
    assert_eq!(
        p.parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str()),
        Some(AUDIO64),
    );
}

// ---------------------------------------------------------------------------
// ffmpeg `-progress pipe:1` parser
// ---------------------------------------------------------------------------

#[test]
fn parse_ffmpeg_progress_us_extracts_out_time_us() {
    assert_eq!(
        parse_ffmpeg_progress_us("out_time_us=12500000"),
        Some(12_500_000)
    );
}

#[test]
fn parse_ffmpeg_progress_us_accepts_legacy_out_time_ms_alias() {
    // `out_time_ms` in ffmpeg's progress stream is a long-standing
    // misnomer — it actually carries microseconds. Keep accepting it so
    // the parser works on older ffmpeg builds too.
    assert_eq!(
        parse_ffmpeg_progress_us("out_time_ms=7000000"),
        Some(7_000_000)
    );
}

#[test]
fn parse_ffmpeg_progress_us_returns_none_for_unrelated_keys() {
    // The progress stream interleaves dozens of `frame=`, `bitrate=`,
    // `progress=continue` lines per second. They must all be ignored
    // so the heartbeat fires only on real time advances.
    assert_eq!(parse_ffmpeg_progress_us("frame=42"), None);
    assert_eq!(parse_ffmpeg_progress_us("bitrate=64.5kbits/s"), None);
    assert_eq!(parse_ffmpeg_progress_us("progress=continue"), None);
    assert_eq!(parse_ffmpeg_progress_us(""), None);
    assert_eq!(parse_ffmpeg_progress_us("no_equals_sign"), None);
}

#[test]
fn parse_ffmpeg_progress_us_returns_none_for_na_warmup_value() {
    // ffmpeg emits `out_time_us=N/A` during the initial warmup before any
    // output frames; treating that as 0 would clobber our `0.01` bootstrap
    // back down to 0.0 mid-startup.
    assert_eq!(parse_ffmpeg_progress_us("out_time_us=N/A"), None);
}

#[test]
fn parse_ffmpeg_progress_us_walks_a_synthetic_progress_stream() {
    // Drive the parser with the exact wire shape ffmpeg emits — interleaved
    // status keys between each `out_time_us=` tick — and assert the parser
    // extracts the time-position values in order while ignoring noise.
    let stream = "\
        frame=10\n\
        fps=10\n\
        bitrate=64.0kbits/s\n\
        out_time_us=1000000\n\
        progress=continue\n\
        frame=20\n\
        out_time_us=2500000\n\
        progress=continue\n\
        out_time_us=5000000\n\
        progress=end\n";
    let parsed: Vec<u64> = stream
        .lines()
        .filter_map(parse_ffmpeg_progress_us)
        .collect();
    assert_eq!(parsed, vec![1_000_000, 2_500_000, 5_000_000]);
}

#[test]
fn ffmpeg_progress_fraction_advances_with_out_time_us() {
    // 60 s elapsed on a 600 s total → 0.1.
    let pct = ffmpeg_progress_fraction(60_000_000, 600.0);
    assert!((pct - 0.1).abs() < 0.001, "expected ~0.1 got {pct}");
}

#[test]
fn ffmpeg_progress_fraction_caps_at_zero_point_nine_five() {
    // Even if ffmpeg overshoots the duration (concat math, container
    // padding) the live sentinel must never reach 1.0 — that value is
    // reserved for the success branch.
    let pct = ffmpeg_progress_fraction(999_999_999, 1.0);
    assert!(pct <= 0.95, "expected clamp at 0.95, got {pct}");
}

#[test]
fn ffmpeg_progress_fraction_floors_at_zero_point_zero_one() {
    // Even at out_time_us=0 the sentinel must show motion so the orphan
    // detector and the status `progress < 0.05` gate both stay coherent.
    let pct = ffmpeg_progress_fraction(0, 600.0);
    assert!(pct >= 0.01, "expected floor at 0.01, got {pct}");
}

#[test]
fn ffmpeg_progress_fraction_uses_safe_default_when_total_unknown() {
    // No probed duration on the parts (audiobook indexed before the lofty
    // pass landed); the heartbeat still has to publish *something* > 0
    // so the orphan detector sees motion.
    let pct = ffmpeg_progress_fraction(123_456, 0.0);
    assert!(
        pct > 0.0 && pct < 0.5,
        "expected small but nonzero, got {pct}"
    );
}

#[test]
fn ffmpeg_progress_fraction_returns_safe_default_for_non_finite_total() {
    // Corrupt tag data can surface NaN/inf as the parts' summed
    // duration. `<= 0.0` doesn't catch NaN, so without the explicit
    // `is_finite()` guard the function would return NaN and leak it
    // into the on-disk `.progress` sentinel.
    let pct = ffmpeg_progress_fraction(60_000_000, f64::NAN);
    assert!(pct.is_finite() && pct > 0.0, "expected finite, got {pct}");
    let pct = ffmpeg_progress_fraction(60_000_000, f64::INFINITY);
    assert!(pct.is_finite() && pct > 0.0, "expected finite, got {pct}");
    let pct = ffmpeg_progress_fraction(60_000_000, f64::NEG_INFINITY);
    assert!(pct.is_finite() && pct > 0.0, "expected finite, got {pct}");
}

#[test]
fn build_manifest_returns_stub_for_non_finite_total_secs() {
    // A NaN/inf summed duration from corrupt tag data must NOT make
    // build_manifest emit `#EXTINF:NaN` or loop over an arbitrary
    // segment count — it has to fall back to the minimal stub.
    for d in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let m = build_manifest(&[HlsPart {
            ordinal: 0,
            filename: "track.mp3".into(),
            duration_seconds: d,
        }]);
        assert!(!m.contains("NaN"), "manifest leaked NaN for {d}: {m}");
        assert!(!m.contains("inf"), "manifest leaked inf for {d}: {m}");
        assert!(m.contains("seg-0000.ts"), "stub missing for {d}: {m}");
        assert!(m.contains("#EXT-X-ENDLIST"), "stub missing for {d}: {m}");
    }
}

#[test]
fn build_manifest_returns_stub_when_segment_count_exceeds_cap() {
    // 100 hours = 36000 segments is the documented cap. Anything past
    // it is corrupt tag data, and we'd rather serve the stub than
    // allocate ~100MB of `#EXTINF:` text. 200 hours blows the cap.
    let m = build_manifest(&[HlsPart {
        ordinal: 0,
        filename: "track.mp3".into(),
        duration_seconds: 200.0 * 3600.0,
    }]);
    assert!(m.contains("seg-0000.ts"));
    assert!(!m.contains("seg-0001.ts"), "over-cap manifest leaked: {m}");
}

// ---------------------------------------------------------------------------
// Orphan-progress recovery (Bug 1 of #338)
// ---------------------------------------------------------------------------

#[test]
fn read_progress_treats_orphan_sentinel_as_zero() {
    // Write `.progress=0.1` and backdate its mtime so it looks like a
    // crashed transcode from minutes ago. `read_progress` must return
    // 0.0 so the status handler's `progress < 0.05` re-kick gate fires
    // on the next poll.
    let (_guard, _dir) = data_dir_guard();
    let book_id = 1234;
    let dir = segment_dir(book_id, AUDIO64);
    std::fs::create_dir_all(&dir).unwrap();
    let path = progress_path(book_id, AUDIO64);
    std::fs::write(&path, "0.1").unwrap();
    backdate(&path, Duration::from_secs(600));

    assert_eq!(
        read_progress(book_id, AUDIO64),
        0.0,
        "stale orphan sentinel must read as 0.0 so the re-kick gate fires"
    );
}

#[test]
fn read_progress_returns_one_for_completed_transcode_even_if_old() {
    // Mtime-based staleness must not poison the success sentinel — a
    // book sitting idle in cache for hours still needs to read as
    // ready.
    let (_guard, _dir) = data_dir_guard();
    let book_id = 2345;
    let dir = segment_dir(book_id, AUDIO64);
    std::fs::create_dir_all(&dir).unwrap();
    let path = progress_path(book_id, AUDIO64);
    std::fs::write(&path, "1.0").unwrap();
    backdate(&path, Duration::from_secs(600));

    assert!(
        is_ready(book_id, AUDIO64),
        "completed sentinel must remain ready regardless of mtime"
    );
}

#[test]
fn read_progress_preserves_fresh_live_progress() {
    // A heartbeat written seconds ago must NOT be treated as orphaned —
    // the threshold gives the live ffmpeg several heartbeats of headroom.
    let (_guard, _dir) = data_dir_guard();
    let book_id = 3456;
    let dir = segment_dir(book_id, AUDIO64);
    std::fs::create_dir_all(&dir).unwrap();
    let path = progress_path(book_id, AUDIO64);
    std::fs::write(&path, "0.42").unwrap();
    // No backdating: mtime is "now". Must read back as the live value.
    let got = read_progress(book_id, AUDIO64);
    assert!((got - 0.42).abs() < 0.01, "expected fresh 0.42, got {got}");
}

#[tokio::test]
async fn transcode_book_clears_orphan_progress_on_restart() {
    // The Bug 1 scenario: a previous run wrote `.progress=0.1`, then the
    // server restarted (or hot-reloaded). On the next worker dispatch,
    // `transcode_book` must clear the orphan before deciding whether to
    // run ffmpeg — otherwise the status endpoint stays stuck because
    // `progress = 0.1 >= 0.05` suppresses the re-kick.
    //
    // We can't drive a real ffmpeg in CI, so we point OMNIBUS_FFMPEG_PATH
    // at a binary that won't exist and assert the only invariant that
    // matters for the bug: the orphan sentinel is *gone* afterwards.
    let (_guard, _dir) = data_dir_guard();
    let book_id = 4567;
    let dir = segment_dir(book_id, AUDIO64);
    std::fs::create_dir_all(&dir).unwrap();
    let path = progress_path(book_id, AUDIO64);
    std::fs::write(&path, "0.1").unwrap();
    backdate(&path, Duration::from_secs(600));

    // Seed minimal book + book_file + book_file_parts rows so
    // `get_parts` returns at least one entry — otherwise `transcode_book`
    // short-circuits before the orphan check would matter.
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind("/lib")
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    sqlx::query(
        "INSERT INTO books (id, uuid, library_id, path, title) VALUES (?, 'u', ?, '/lib', 't')",
    )
    .bind(book_id)
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap();
    let file_id = sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'MP3', 'b', 0)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_file_parts (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds) \
         VALUES (?, 0, 'p.mp3', 0, 0, 1.0)",
    )
    .bind(file_id)
    .execute(&pool)
    .await
    .unwrap();

    // Point ffmpeg at a binary that does not exist — `transcode_book`
    // bails on spawn but only AFTER it has wiped the orphan sentinel
    // and written the bootstrap `0.01`. The cleanup path that follows
    // also nukes the directory, which is fine: either branch ends with
    // `read_progress` below 0.05.
    //
    // `_guard` already holds ENV_LOCK; `also_set` adds OMNIBUS_FFMPEG_PATH
    // under the same lock so it is restored automatically on drop.
    let _guard = _guard.also_set(
        "OMNIBUS_FFMPEG_PATH",
        Some("/this/binary/does/not/exist/ffmpeg"),
    );

    let _ = super::transcode_book(&pool, book_id, file_id, "/lib", AUDIO64).await;

    let observed = read_progress(book_id, AUDIO64);
    assert!(
        observed < 0.05,
        "orphan 0.1 sentinel must have been cleared; got {observed}"
    );
}

#[tokio::test]
async fn get_parts_propagates_db_error_when_pool_is_closed() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = get_parts(&pool, 1).await.unwrap_err();
    assert!(matches!(err, HlsError::Db(_)));
}

#[test]
fn used_bytes_sums_every_segment_file_across_book_directories() {
    let (_guard, _dir) = data_dir_guard();
    let base = hls_dir();
    let book0 = base.join("0").join(AUDIO64);
    let book1 = base.join("1").join(AUDIO64);
    std::fs::create_dir_all(&book0).unwrap();
    std::fs::create_dir_all(&book1).unwrap();
    std::fs::write(book0.join("seg0.ts"), vec![0u8; 100]).unwrap();
    std::fs::write(book1.join("seg0.ts"), vec![0u8; 50]).unwrap();
    std::fs::write(book1.join("seg1.ts"), vec![0u8; 25]).unwrap();

    assert_eq!(used_bytes(), 175);
}

#[test]
fn used_bytes_is_zero_when_the_hls_dir_does_not_exist() {
    let (_guard, _dir) = data_dir_guard();
    // `data_dir_guard` points `OMNIBUS_DATA_DIR` at a fresh tempdir, but
    // `hls_dir()` (`<data_dir>/hls`) itself is never created until the
    // first transcode — assert the pre-first-run state reads as empty.
    assert!(!hls_dir().exists());
    assert_eq!(used_bytes(), 0);
}

/// Mirrors `thumbs::tests::evict_if_over_cap_removes_oldest_files`'s shape:
/// FIFO-by-mtime eviction, but scoped to whole `<book_id>/<profile>/` segment
/// directories rather than individual thumbnail files.
#[test]
fn evict_if_over_cap_removes_oldest_book_directory() {
    let (_guard, _dir) = data_dir_guard();
    let base = hls_dir();

    // Three book directories with staggered mtimes (we can only guarantee
    // ordering, not specific times, so create sequentially and trust OS
    // mtime, as the thumbs test does).
    for i in 0u8..3 {
        let book_dir = base.join(i.to_string()).join(AUDIO64);
        std::fs::create_dir_all(&book_dir).unwrap();
        std::fs::write(book_dir.join("seg0.ts"), vec![0u8; 100]).unwrap();
        // Small sleep to ensure distinct mtimes on coarse-resolution filesystems.
        std::thread::sleep(Duration::from_millis(50));
    }

    // Cap at 200 bytes → should delete the 1 oldest book dir (100 bytes each,
    // 3x100 = 300 total).
    evict_if_over_cap(200).unwrap();

    let remaining: Vec<_> = std::fs::read_dir(&base).unwrap().flatten().collect();
    assert_eq!(remaining.len(), 2, "should have evicted 1 oldest book dir");
    assert!(
        !base.join("0").exists(),
        "the oldest book dir (0) should be evicted"
    );
}
