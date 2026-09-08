//! One-off task kinds: `SendToKindle` without SMTP, `RewriteAllEpubs`
//! reporting per-book bake errors through its outcome, `ConvertFormat`
//! with a missing binary, a timeout and the convert concurrency cap, and
//! the `WorkerConfig` default for that cap.

use std::os::unix::fs::PermissionsExt;

use omnibus_shared::MetadataOverrides;
use sqlx::SqlitePool;
use tempfile::TempDir;

use crate::ebook::test_support::copy_fixture_into;
use crate::epub_rewrite::tests::seed_epub_row;
use crate::test_support::EnvVarGuard;

use super::super::types::{Task, TaskOutcome, TaskSuccessDetail, Worker, WorkerConfig};
use super::{make_worker_default, poll_maps_empty, pool};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_to_kindle_task_fails_when_smtp_unconfigured() {
    // Route through the real dispatch arm: with no SMTP config the handler
    // returns a Failed outcome carrying the "not configured" message.
    let _env =
        crate::test_support::EnvVarGuard::set("SMTP_HOST", None).also_set("SMTP_FROM_EMAIL", None);
    let w = make_worker_default(pool().await);
    let id = w.post(Task::SendToKindle {
        book_id: 1,
        book_file_id: None,
        recipient_email: "reader@kindle.com".into(),
    });
    match w.await_completion(id).await {
        TaskOutcome::Err(msg) => assert!(msg.contains("not configured"), "got: {msg}"),
        other => panic!("expected Err, got {other:?}"),
    }
}

/// A `Task::RewriteAllEpubs` run that leaves one book unbaked (its
/// `book_files` row points at a source that was never written to disk)
/// completes as `TaskOutcome::Ok`, and the failed `book_uuid` — the error
/// text itself stays server-side, only logged — rides through as
/// `TaskSuccessDetail::BakeErrors` (#1739). A book with a real fixture on
/// disk bakes successfully alongside it, so the run doesn't abort on the
/// first failure.
/// Seed the two-book fixture a `Task::RewriteAllEpubs` run needs to leave
/// exactly one book unbaked: `uuid-ok` has a real EPUB on disk, `uuid-bad`
/// points at a source that was never written. The returned guards redirect
/// the export cache and own the library dirs, so the caller must hold them
/// for the life of the test.
async fn seed_one_failing_bake(pool: &SqlitePool) -> (EnvVarGuard, TempDir, TempDir, TempDir) {
    let export = tempfile::tempdir().unwrap();
    let env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let user_id = crate::auth::create_user(pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    let lib_ok = tempfile::tempdir().unwrap();
    copy_fixture_into("alpha.epub", lib_ok.path());
    seed_epub_row(pool, lib_ok.path(), "uuid-ok", "Book OK", "alpha").await;
    crate::upsert_metadata_overrides(
        pool,
        "uuid-ok",
        &MetadataOverrides {
            title: Some("Fixed".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let lib_bad = tempfile::tempdir().unwrap();
    seed_epub_row(pool, lib_bad.path(), "uuid-bad", "Book Bad", "missing").await;
    crate::upsert_metadata_overrides(
        pool,
        "uuid-bad",
        &MetadataOverrides {
            title: Some("Never Applied".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    (env, export, lib_ok, lib_bad)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_rewrite_all_epubs_reports_bake_errors_via_task_outcome() {
    let pool = pool().await;
    let _guards = seed_one_failing_bake(&pool).await;

    let w = make_worker_default(pool);
    let id = w.post(Task::RewriteAllEpubs);
    match w.await_completion(id).await {
        TaskOutcome::Ok(Some(TaskSuccessDetail::BakeErrors(errors))) => {
            assert_eq!(errors, vec!["uuid-bad".to_string()]);
        }
        other => panic!("expected Ok with bake errors, got {other:?}"),
    }
}

/// The [`TaskSuccessDetail`] survives the completion slot being reclaimed
/// first, so a late awaiter still learns *which* books failed rather than a
/// bare `Ok(None)` — the detail-bearing counterpart to
/// [`await_completion_returns_the_outcome_after_the_slot_was_pruned`].
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_rewrite_all_epubs_reports_bake_errors_after_the_slot_was_pruned() {
    let pool = pool().await;
    let _guards = seed_one_failing_bake(&pool).await;

    let w = make_worker_default(pool);
    let id = w.post(Task::RewriteAllEpubs);
    assert!(poll_maps_empty(&w).await, "bake never finished");
    match w.await_completion(id).await {
        TaskOutcome::Ok(Some(TaskSuccessDetail::BakeErrors(errors))) => {
            assert_eq!(errors, vec!["uuid-bad".to_string()]);
        }
        other => panic!("expected the retained bake errors, got {other:?}"),
    }
}

/// A `Task::RewriteAllEpubs` run with nothing to bake (no override rows at
/// all) reports the ordinary `TaskOutcome::Ok(None)` — the same shape every
/// other error-free task kind reports.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_rewrite_all_epubs_reports_ok_none_when_nothing_fails() {
    let w = make_worker_default(pool().await);
    let id = w.post(Task::RewriteAllEpubs);
    assert!(matches!(
        w.await_completion(id).await,
        TaskOutcome::Ok(None)
    ));
}

/// A `Task::RewriteAllEpubs` run against a closed pool can't even resolve
/// the batch — that failure reaches `handle_rewrite_all_epubs`'s `Err` arm
/// and comes back as a sanitized `TaskOutcome::Err`, never the raw
/// `sqlx`/`BooksError` text.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_rewrite_all_epubs_reports_sanitized_err_when_the_pool_is_closed() {
    let pool = pool().await;
    pool.close().await;

    let w = make_worker_default(pool);
    let id = w.post(Task::RewriteAllEpubs);
    // `RewriteAllEpubs` has no `on_run` hook to gate, and against a closed
    // pool it fails (and prunes its completions slot) almost instantly, so
    // without draining first this races `await_completion` grabbing the
    // live receiver against the prune. Forcing the pruned-slot interleaving
    // here, as in `await_completion_returns_the_outcome_after_the_slot_was_pruned`,
    // makes the test deterministically exercise the retained-outcome path.
    assert!(poll_maps_empty(&w).await, "task never finished");
    match w.await_completion(id).await {
        TaskOutcome::Err(msg) => {
            assert!(msg.contains("epub override bake"), "{msg}");
            assert!(!msg.to_lowercase().contains("sqlx"), "{msg}");
        }
        other => panic!("expected Err, got {other:?}"),
    }
}

// Task::ConvertFormat (#948)
/// Write a fake `ebook-convert` at `path`: handles `--version`, otherwise
/// mimics the real `ebook-convert <src> <out>` positional invocation by
/// copying `$1` → `$2`.
fn write_copying_ebook_convert(path: &std::path::Path) {
    let script = "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then echo 'ebook-convert 0-fake'; exit 0; fi\n\
         cp \"$1\" \"$2\"\n";
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Write a fake `ebook-convert` that answers `--version` immediately but
/// sleeps past any short test timeout on a real invocation.
fn write_slow_ebook_convert(path: &std::path::Path) {
    let script = "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then echo 'ebook-convert 0-fake'; exit 0; fi\n\
         sleep 5\n\
         cp \"$1\" \"$2\"\n";
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Write a fake `ebook-convert` that atomically `mkdir`s a lock directory
/// before "converting" and `rmdir`s it after, appending to `collisions` if
/// the directory already existed — i.e. if a peer invocation was already
/// mid-run. `mkdir` is atomic on POSIX filesystems, so this is a reliable
/// concurrent-invocation detector without any Rust-side synchronization.
fn write_locking_ebook_convert(
    path: &std::path::Path,
    lockdir: &std::path::Path,
    collisions: &std::path::Path,
) {
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then echo 'ebook-convert 0-fake'; exit 0; fi\n\
         if ! mkdir '{lockdir}' 2>/dev/null; then\n\
         echo collision >> '{collisions}'\n\
         else\n\
         sleep 0.2\n\
         rmdir '{lockdir}'\n\
         fi\n\
         cp \"$1\" \"$2\"\n",
        lockdir = lockdir.display(),
        collisions = collisions.display(),
    );
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// AC1/AC4 happy path: a valid `(source_format, target_format)` pair
/// completes with a successful [`TaskOutcome`].
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn convert_format_task_completes_with_ok_outcome_for_a_valid_pair() {
    let pool = pool().await;
    let lib = tempfile::tempdir().unwrap();
    let book_id = crate::test_support::seed_epub_book_at(&pool, lib.path())
        .await
        .0;

    let cache = tempfile::tempdir().unwrap();
    let bin_dir = tempfile::tempdir().unwrap();
    let script = bin_dir.path().join("ebook-convert");
    write_copying_ebook_convert(&script);
    let _env = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(cache.path().as_os_str()))
        .also_set_os("OMNIBUS_EBOOK_CONVERT_PATH", Some(script.as_os_str()));

    let w = make_worker_default(pool);
    let id = w.post(Task::ConvertFormat {
        book_id,
        source_format: "EPUB".into(),
        target_format: "MOBI".into(),
    });
    match w.await_completion(id).await {
        TaskOutcome::Ok(_) => {}
        other => panic!("expected Ok, got {other:?}"),
    }
}

/// AC4 failure path: a missing/non-runnable `ebook-convert` binary reports a
/// specific, client-facing failure rather than hanging or a generic error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn convert_format_task_reports_failure_when_binary_is_missing() {
    let pool = pool().await;
    let lib = tempfile::tempdir().unwrap();
    let book_id = crate::test_support::seed_epub_book_at(&pool, lib.path())
        .await
        .0;
    let cache = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(cache.path().as_os_str())).also_set(
        "OMNIBUS_EBOOK_CONVERT_PATH",
        Some("/nonexistent/omnibus-ebook-convert-probe"),
    );

    let w = make_worker_default(pool);
    let id = w.post(Task::ConvertFormat {
        book_id,
        source_format: "EPUB".into(),
        target_format: "MOBI".into(),
    });
    match w.await_completion(id).await {
        TaskOutcome::Err(msg) => assert!(
            msg.contains("not installed") || msg.contains("not runnable"),
            "got {msg}"
        ),
        other => panic!("expected Err, got {other:?}"),
    }
}

/// AC3/AC4: a job that outruns `OMNIBUS_CONVERT_TIMEOUT_SECS` aborts and
/// reports failure rather than hanging the worker.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn convert_format_task_reports_failure_on_timeout() {
    let pool = pool().await;
    let lib = tempfile::tempdir().unwrap();
    let book_id = crate::test_support::seed_epub_book_at(&pool, lib.path())
        .await
        .0;
    let cache = tempfile::tempdir().unwrap();
    let bin_dir = tempfile::tempdir().unwrap();
    let script = bin_dir.path().join("ebook-convert");
    write_slow_ebook_convert(&script);
    let _env = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(cache.path().as_os_str()))
        .also_set_os("OMNIBUS_EBOOK_CONVERT_PATH", Some(script.as_os_str()))
        .also_set("OMNIBUS_CONVERT_TIMEOUT_SECS", Some("1"));

    let w = make_worker_default(pool);
    let id = w.post(Task::ConvertFormat {
        book_id,
        source_format: "EPUB".into(),
        target_format: "MOBI".into(),
    });
    match w.await_completion(id).await {
        TaskOutcome::Err(msg) => assert!(msg.contains("timed out"), "got {msg}"),
        other => panic!("expected Err, got {other:?}"),
    }
}

/// AC2: two conversions for the same book (different `target_format`, so
/// they don't share a resource key and would otherwise run in parallel) never
/// overlap when `convert_concurrency=1` — proven by an atomic `mkdir`-based
/// lock in the fake binary rather than racing a real subprocess on timing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn convert_format_task_respects_the_convert_concurrency_cap() {
    let pool = pool().await;
    let lib = tempfile::tempdir().unwrap();
    let book_id = crate::test_support::seed_epub_book_at(&pool, lib.path())
        .await
        .0;

    let cache = tempfile::tempdir().unwrap();
    let bin_dir = tempfile::tempdir().unwrap();
    let script = bin_dir.path().join("ebook-convert");
    let lockdir = bin_dir.path().join("lock");
    let collisions = bin_dir.path().join("collisions");
    write_locking_ebook_convert(&script, &lockdir, &collisions);
    let _env = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(cache.path().as_os_str()))
        .also_set_os("OMNIBUS_EBOOK_CONVERT_PATH", Some(script.as_os_str()));

    let w = Worker::new(
        pool,
        WorkerConfig {
            scan_concurrency: 1,
            hls_concurrency: 1,
            convert_concurrency: 1,
        },
    );

    let id1 = w.post(Task::ConvertFormat {
        book_id,
        source_format: "EPUB".into(),
        target_format: "MOBI".into(),
    });
    let id2 = w.post(Task::ConvertFormat {
        book_id,
        source_format: "EPUB".into(),
        target_format: "AZW3".into(),
    });

    let (out1, out2) = tokio::join!(w.await_completion(id1), w.await_completion(id2));
    assert!(matches!(out1, TaskOutcome::Ok(_)), "got {out1:?}");
    assert!(matches!(out2, TaskOutcome::Ok(_)), "got {out2:?}");
    assert!(
        !collisions.exists(),
        "convert_concurrency=1 must serialize the two conversions"
    );
}

/// [`WorkerConfig::default`]'s `convert_concurrency` follows the same
/// `max(1, num_cpus / 2)` formula as `hls_concurrency` (#948 AC2) — the two
/// fields are computed from the same `num_cpus()` call, so they must agree
/// regardless of how many CPUs the test host has.
#[test]
fn worker_config_default_convert_concurrency_matches_the_hls_concurrency_formula() {
    let cfg = WorkerConfig::default();
    assert_eq!(cfg.convert_concurrency, cfg.hls_concurrency);
    assert!(cfg.convert_concurrency >= 1);
}
