//! The progress callbacks `reindex_with_progress` /
//! `reindex_audiobooks_with_progress` emit: phase order, per-phase tallies,
//! and library-relative current-item paths.

use crate::pool::init_db;
use crate::sync::{sync_audiobooks, AudiobookSyncPlan};
use crate::test_support::{make_test_dir, CoversTempDir};

use super::super::*;
use super::seed_ebook_at;

/// The verbose progress stream: a walking-phase event opens the scan,
/// parse and sync events carry the diff's tallies plus the current item,
/// and every reported path is the library directory name plus the
/// relative path — never the absolute server path.
#[tokio::test]
async fn reindex_with_progress_reports_phases_tallies_and_relative_current_items() {
    let _covers = CoversTempDir::new("reindex-verbose");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("reindex-verbose-lib");
    let lib_path = lib.to_string_lossy().into_owned();
    let root_name = lib.file_name().unwrap().to_string_lossy().into_owned();

    seed_ebook_at(&pool, &lib_path, "a.epub", "Dracula").await;
    seed_ebook_at(&pool, &lib_path, "b.epub", "Frankenstein").await;
    // A file on disk with no DB row — the New bucket the parse + sync
    // phases will report on.
    std::fs::write(lib.join("fresh.epub"), b"stub").unwrap();

    let updates: std::sync::Arc<std::sync::Mutex<Vec<ScanUpdate>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let updates_cb = updates.clone();
    reindex_with_progress(&pool, &lib_path, move |u| {
        updates_cb.lock().unwrap().push(u);
    })
    .await
    .unwrap();

    let updates = updates.lock().unwrap();
    assert_eq!(
        updates.first().and_then(|u| u.detail.phase.as_deref()),
        Some(PHASE_WALKING),
        "the walking phase must open the stream"
    );

    let parse_event = updates
        .iter()
        .find(|u| u.detail.phase.as_deref() == Some(PHASE_PARSING))
        .expect("a parse-phase event for the New file");
    let item = parse_event.detail.current_item.as_deref().unwrap();
    assert_eq!(item, format!("{root_name}/fresh.epub"));
    let tallies = parse_event
        .detail
        .tallies
        .expect("parse events carry tallies");
    assert_eq!(tallies.found, 3);
    assert_eq!(tallies.new, 1);
    assert_eq!(tallies.unchanged, 2);

    let sync_event = updates
        .iter()
        .find(|u| {
            u.detail.phase.as_deref() == Some(PHASE_SYNCING) && u.detail.current_item.is_some()
        })
        .expect("a sync-phase event naming the written book");
    assert_eq!(
        sync_event.detail.current_item.as_deref().unwrap(),
        format!("{root_name}/fresh.epub")
    );

    for u in updates.iter() {
        if let Some(item) = u.detail.current_item.as_deref() {
            assert!(
                !item.contains(&lib_path),
                "current_item leaked the absolute library path: {item}"
            );
        }
    }

    let _ = std::fs::remove_dir_all(&lib);
}

/// Index one M4B audiobook through the real `sync_audiobooks` write path at
/// `library_path` (a real on-disk dir), writing a matching stub file so a
/// later reindex re-finds it and classifies it Unchanged. `filename` is the
/// library-relative path — a single-file `.m4b` group is its own book, so
/// `filename` doubles as both the group's `scan_key` and its one part.
async fn seed_audiobook_at(pool: &SqlitePool, library_path: &str, filename: &str, title: &str) {
    let abs = std::path::Path::new(library_path).join(filename);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&abs, b"stub").unwrap();
    let (mtime, size) = {
        let meta = std::fs::metadata(&abs).unwrap();
        (
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            meta.len() as i64,
        )
    };
    let book = crate::audiobook::IndexedAudiobook {
        scan_key: filename.to_string(),
        group_path: filename.to_string(),
        format: "M4B".to_string(),
        title: title.to_string(),
        creator_name: Some("Author".to_string()),
        cover: None,
        accent: None,
        parts: vec![crate::audiobook::AudiobookPart {
            ordinal: 0,
            filename: filename.to_string(),
            size_bytes: size,
            mtime_epoch: mtime,
            duration_seconds: 3600.0,
        }],
        chapters: vec![],
        total_size_bytes: size,
        max_mtime_epoch: mtime,
        description: None,
        error: None,
    };
    sync_audiobooks(
        pool,
        library_path,
        AudiobookSyncPlan {
            new_books: vec![book],
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

/// The verbose progress stream's audiobook-pipeline sibling to
/// `reindex_with_progress_reports_phases_tallies_and_relative_current_items`:
/// a walking-phase event opens the scan, parse and sync events carry the
/// diff's tallies plus the current item, and every reported path is the
/// library directory name plus the relative path — never the absolute
/// server path.
#[tokio::test]
async fn reindex_audiobooks_with_progress_reports_phases_tallies_and_relative_current_items() {
    let _covers = CoversTempDir::new("reindex-audio-verbose");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("reindex-audio-verbose-lib");
    let lib_path = lib.to_string_lossy().into_owned();
    let root_name = lib.file_name().unwrap().to_string_lossy().into_owned();

    seed_audiobook_at(&pool, &lib_path, "dracula.m4b", "Dracula").await;
    seed_audiobook_at(&pool, &lib_path, "frankenstein.m4b", "Frankenstein").await;
    // A file on disk with no DB row — the New bucket the parse + sync
    // phases will report on. Not a real M4B container, so lofty's tag read
    // fails and the parser falls back to zero-duration defaults (a WARN,
    // not an error) — same tolerance a corrupt file gets in a real library.
    std::fs::write(lib.join("fresh.m4b"), b"stub").unwrap();

    let updates: std::sync::Arc<std::sync::Mutex<Vec<ScanUpdate>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let updates_cb = updates.clone();
    reindex_audiobooks_with_progress(&pool, &lib_path, move |u| {
        updates_cb.lock().unwrap().push(u);
    })
    .await
    .unwrap();

    let updates = updates.lock().unwrap();

    // Phase ordering: walking opens the stream, and every parse event's
    // index precedes every sync event's index — not just "a parse event
    // exists somewhere and a sync event exists somewhere", which would
    // pass even if the phases interleaved out of order.
    let walking_idx = updates
        .iter()
        .position(|u| u.detail.phase.as_deref() == Some(PHASE_WALKING))
        .expect("a walking-phase event");
    assert_eq!(walking_idx, 0, "the walking phase must open the stream");

    let parse_indices: Vec<usize> = updates
        .iter()
        .enumerate()
        .filter(|(_, u)| u.detail.phase.as_deref() == Some(PHASE_PARSING))
        .map(|(i, _)| i)
        .collect();
    assert!(
        !parse_indices.is_empty(),
        "expected at least one parse-phase event for the New file"
    );
    let sync_indices: Vec<usize> = updates
        .iter()
        .enumerate()
        .filter(|(_, u)| u.detail.phase.as_deref() == Some(PHASE_SYNCING))
        .map(|(i, _)| i)
        .collect();
    assert!(
        !sync_indices.is_empty(),
        "expected at least one sync-phase event"
    );
    let last_parse_idx = *parse_indices.last().unwrap();
    let first_sync_idx = sync_indices[0];
    assert!(
        walking_idx < parse_indices[0],
        "walking ({walking_idx}) must precede parsing ({})",
        parse_indices[0]
    );
    assert!(
        last_parse_idx < first_sync_idx,
        "every parse event ({last_parse_idx} last) must precede every sync event \
         ({first_sync_idx} first) — PHASE_WALKING -> PHASE_PARSING -> PHASE_SYNCING"
    );

    // Tallies are fixed once the diff lands (see reindex_audiobooks_with_progress's
    // doc comment), so every parse event — not just one sampled event — must
    // carry the identical ScanTallies, and every sync event must carry the
    // same tallies too.
    let expected_tallies = updates[parse_indices[0]]
        .detail
        .tallies
        .expect("parse events carry tallies");
    assert_eq!(expected_tallies.found, 3);
    assert_eq!(expected_tallies.new, 1);
    assert_eq!(expected_tallies.unchanged, 2);
    for &i in &parse_indices {
        assert_eq!(
            updates[i].detail.tallies,
            Some(expected_tallies),
            "parse event at index {i} carried different tallies than the first parse event"
        );
    }
    for &i in &sync_indices {
        assert_eq!(
            updates[i].detail.tallies,
            Some(expected_tallies),
            "sync event at index {i} carried different tallies than the parse phase"
        );
    }
    let parse_item = updates[parse_indices[0]]
        .detail
        .current_item
        .as_deref()
        .unwrap();
    assert_eq!(parse_item, format!("{root_name}/fresh.m4b"));

    let sync_named_idx = sync_indices
        .iter()
        .copied()
        .find(|&i| updates[i].detail.current_item.is_some())
        .expect("a sync-phase event naming the written book");
    assert_eq!(
        updates[sync_named_idx]
            .detail
            .current_item
            .as_deref()
            .unwrap(),
        format!("{root_name}/fresh.m4b")
    );

    for u in updates.iter() {
        if let Some(item) = u.detail.current_item.as_deref() {
            assert!(
                !item.contains(&lib_path),
                "current_item leaked the absolute library path: {item}"
            );
        }
    }

    let _ = std::fs::remove_dir_all(&lib);
}
