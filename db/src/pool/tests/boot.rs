//! Boot-time behaviour: `init_db` on a bad URL or a tampered applied
//! checksum, the migrator recording versions and being idempotent, and the
//! one-time legacy cover purge with its missing-dir and unremovable-entry
//! paths.

use super::super::*;

use crate::test_support::CoversTempDir;

#[tokio::test]
async fn init_db_returns_db_error_when_url_is_invalid() {
    // `sqlite://` URL pointing at a path under a non-existent dir + no
    // `mode=rwc` flag forces the underlying pool connect to fail. The
    // typed wrapper must surface a `Db` variant rather than panicking
    // or leaking `sqlx::Error` at the signature.
    let err = init_db("sqlite:///nonexistent/dir/omnibus.db")
        .await
        .expect_err("invalid url should fail to open");
    assert!(matches!(err, InitDbError::Db(_)));
}

#[tokio::test]
async fn init_db_returns_migrate_error_when_applied_checksum_is_tampered() {
    // sqlx checksums every migration and, at startup, compares each embedded
    // migration against the checksum recorded in `_sqlx_migrations`. A row
    // whose stored checksum no longer matches (an edited-after-apply migration,
    // per rule 06) must fail startup — the typed wrapper surfaces it as
    // `InitDbError::Migrate`, not a panic or a leaked `MigrateError`.
    //
    // First `init_db` lets sqlx create and populate `_sqlx_migrations` itself
    // (no hand-recreation of its internal schema, which would rot across sqlx
    // versions); then we tamper version 1's checksum in place and re-open. The
    // tempdir auto-removes the DB plus its WAL/`-shm` sidecars on drop.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("omnibus.db");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());

    let pool = init_db(&url).await.expect("initial init_db should succeed");
    sqlx::query("UPDATE _sqlx_migrations SET checksum = zeroblob(48) WHERE version = 1")
        .execute(&pool)
        .await
        .unwrap();
    drop(pool);

    let err = init_db(&url)
        .await
        .expect_err("a tampered migration checksum must fail startup");
    assert!(
        matches!(err, InitDbError::Migrate(_)),
        "expected InitDbError::Migrate, got {err:?}"
    );
}

#[tokio::test]
async fn migrator_records_applied_versions() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await
            .expect("_sqlx_migrations should exist after init_db");
    assert!(
        versions.contains(&1),
        "baseline migration 0001 should be recorded, got {versions:?}"
    );
    assert!(
        versions.contains(&2),
        "normalized migration 0002 should be recorded, got {versions:?}"
    );
    assert!(
        versions.contains(&3),
        "legacy-drop migration 0003 should be recorded, got {versions:?}"
    );
}

#[tokio::test]
async fn migrator_is_idempotent_on_rerun() {
    let tmp = std::env::temp_dir().join(format!(
        "omnibus-migrate-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let url = format!("sqlite://{}?mode=rwc", tmp.display());

    let pool1 = init_db(&url).await.expect("first init");
    drop(pool1);
    let pool2 = init_db(&url).await.expect("second init");

    let by_version: Vec<(i64, i64)> =
        sqlx::query_as("SELECT version, COUNT(*) FROM _sqlx_migrations GROUP BY version")
            .fetch_all(&pool2)
            .await
            .unwrap();
    for (_, count) in by_version {
        assert_eq!(count, 1, "every migration recorded exactly once");
    }

    drop(pool2);
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn purge_legacy_covers_once_sweeps_then_no_ops() {
    // `CoversTempDir` pins `OMNIBUS_COVERS_DIR` at a unique temp path under
    // the shared env lock and removes it on drop; the purge takes the dir as
    // a parameter, so this asserts the function in isolation from `init_db`.
    let covers = CoversTempDir::new("purge_sweep");
    std::fs::create_dir_all(&covers.path).unwrap();

    // Seed three "legacy" cover files.
    for name in ["aaaa.jpg", "bbbb.png", "cccc.webp"] {
        std::fs::write(covers.path.join(name), b"x").unwrap();
    }

    purge_legacy_covers_once(&covers.path);

    // Legacy files gone, sentinel written.
    for name in ["aaaa.jpg", "bbbb.png", "cccc.webp"] {
        assert!(
            !covers.path.join(name).exists(),
            "legacy file {name} should have been purged",
        );
    }
    assert!(
        covers.path.join(COVERS_SCHEME_SENTINEL).exists(),
        "sentinel should be present after first purge",
    );

    // A freshly-written cover after the purge must survive a second
    // call — the sentinel short-circuits the sweep.
    let kept = covers.path.join("dddd.jpg");
    std::fs::write(&kept, b"y").unwrap();
    purge_legacy_covers_once(&covers.path);
    assert!(
        kept.exists(),
        "post-sentinel cover writes must not be deleted",
    );
}

#[test]
fn purge_legacy_covers_once_creates_and_marks_a_missing_dir() {
    // Cold boot before any covers have ever been written: nothing to purge,
    // but the scheme still has to be marked, so the dir is created for the
    // sentinel to live in.
    let covers = CoversTempDir::new("purge_missing");
    assert!(
        !covers.path.exists(),
        "precondition: dir does not exist yet"
    );

    purge_legacy_covers_once(&covers.path);

    assert!(
        covers.path.join(COVERS_SCHEME_SENTINEL).exists(),
        "a missing covers dir must still be created and marked",
    );
    let entries: Vec<_> = std::fs::read_dir(&covers.path).unwrap().flatten().collect();
    assert_eq!(
        entries.len(),
        1,
        "the sentinel should be the only thing written",
    );
}

#[test]
fn purge_legacy_covers_once_leaves_covers_extracted_after_a_missing_dir_boot() {
    // The fresh-install regression: boot 1 finds no covers dir, boot 1's
    // indexing run then extracts every cover into it, and boot 2 must not
    // mistake that populated cache for an unswept legacy one.
    let covers = CoversTempDir::new("purge_fresh_install");

    // Boot 1: no dir yet.
    purge_legacy_covers_once(&covers.path);

    // Boot 1's indexer extracts covers into the now-existing dir.
    for name in ["aaaa.jpg", "bbbb.png", "cccc.webp"] {
        std::fs::write(covers.path.join(name), b"x").unwrap();
    }

    // Boot 2.
    purge_legacy_covers_once(&covers.path);

    for name in ["aaaa.jpg", "bbbb.png", "cccc.webp"] {
        assert!(
            covers.path.join(name).exists(),
            "cover {name} extracted after the first boot must survive the second",
        );
    }
}

#[cfg(unix)]
#[test]
fn purge_legacy_covers_once_leaves_scheme_unmarked_when_a_file_cannot_be_removed() {
    use std::os::unix::fs::PermissionsExt;

    // A partial sweep must not write the sentinel: marking a dir that still
    // holds a legacy file would orphan that file forever.
    let covers = CoversTempDir::new("purge_readonly");
    std::fs::create_dir_all(&covers.path).unwrap();
    let stuck = covers.path.join("aaaa.jpg");
    std::fs::write(&stuck, b"x").unwrap();

    // A read-only directory rejects the unlink of an entry inside it.
    std::fs::set_permissions(&covers.path, std::fs::Permissions::from_mode(0o555)).unwrap();
    purge_legacy_covers_once(&covers.path);
    // Restore before asserting so the temp dir is still removable on drop.
    std::fs::set_permissions(&covers.path, std::fs::Permissions::from_mode(0o755)).unwrap();

    // Running as root ignores the mode bits, so the unlink succeeds and there
    // is no partial sweep to assert on.
    if stuck.exists() {
        assert!(
            !covers.path.join(COVERS_SCHEME_SENTINEL).exists(),
            "a sweep that left a legacy file behind must leave the scheme unmarked",
        );
    }
}

#[cfg(unix)]
#[test]
fn purge_legacy_covers_once_leaves_scheme_unmarked_when_an_entry_cannot_be_statted() {
    // An entry the sweep can't stat is one it can't verify or remove, so the
    // sentinel must not land. A dangling symlink is the reproducible case:
    // `std::fs::metadata` follows the link and errors on the missing target.
    let covers = CoversTempDir::new("purge_unstattable");
    std::fs::create_dir_all(&covers.path).unwrap();
    std::os::unix::fs::symlink(
        covers.path.join("no-such-target"),
        covers.path.join("dangling.jpg"),
    )
    .unwrap();

    purge_legacy_covers_once(&covers.path);

    assert!(
        !covers.path.join(COVERS_SCHEME_SENTINEL).exists(),
        "a sweep with an unstattable entry must leave the scheme unmarked",
    );
}
