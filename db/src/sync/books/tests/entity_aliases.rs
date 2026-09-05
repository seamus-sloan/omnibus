//! Merged-away authors, series and tags under a reindex: the alias maps
//! `collect_entity_alias_maps` builds keep a Changed scan from resurrecting
//! them, leave unmerged entities alone, and are fetched once per kind
//! rather than once per book.

use sqlx::SqlitePool;

use super::super::{
    collect_entity_alias_maps, sync_books, sync_changed, sync_new, EntityAliasMaps, SyncPlan,
};
use super::seed_scan_root;
use crate::cleanup::{apply_merge_authors, apply_merge_series, apply_merge_tags};
use crate::ebook::IndexedBook;
use crate::pool::init_db;
use crate::test_support::{
    author_id_by_name, count_rows, indexed, series_id_by_name, CoversTempDir,
};

/// Look up a `tags.id` by name. Panics on miss — test helper only.
async fn tag_id_by_name(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT id FROM tags WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// `true` when `table.name = ? table` has no row for the given name.
async fn taxonomy_row_missing(pool: &SqlitePool, table: &str, name: &str) -> bool {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE name = ?");
    count_rows_bound(pool, &sql, name).await == 0
}

async fn count_rows_bound(pool: &SqlitePool, sql: &str, bind: &str) -> i64 {
    sqlx::query_scalar(sql)
        .bind(bind)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// `true` when `book_id` is linked to `entity_id` in `link_table` via `col`.
async fn is_linked(
    pool: &SqlitePool,
    link_table: &str,
    col: &str,
    book_id: i64,
    entity_id: i64,
) -> bool {
    let sql = format!("SELECT COUNT(*) FROM {link_table} WHERE book = ? AND {col} = ?");
    let n: i64 = sqlx::query_scalar(&sql)
        .bind(book_id)
        .bind(entity_id)
        .fetch_one(pool)
        .await
        .unwrap();
    n == 1
}

/// #964 AC2: merging an author, series, and tag away, then reindexing the
/// book whose file still names the merged-away entities (`sync_changed`,
/// the real `Task::Scan` write path for an unchanged-on-disk-but-reparsed
/// file), must resolve every one of them to the surviving canonical row —
/// never mint a fresh `authors`/`series`/`tags` row for a name a completed
/// merge already absorbed.
#[tokio::test]
async fn sync_changed_reindex_does_not_resurrect_a_merged_away_author_series_or_tag() {
    let _covers = CoversTempDir::new("resurrection_guard");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;

    let source = indexed(
        "source.epub",
        Some("Source Book"),
        &["John Smith"],
        &["Sci-Fi"],
        Some(("The Wheel of Time", "1")),
        None,
    );
    let canonical = indexed(
        "canonical.epub",
        Some("Canonical Book"),
        &["J. Smith"],
        &["Science Fiction"],
        Some(("Wheel of Time", "1")),
        None,
    );
    let mut tx = pool.begin().await.unwrap();
    sync_new(
        &mut tx,
        library_id,
        "/lib",
        &[source, canonical],
        &[],
        &EntityAliasMaps::default(),
        |_| {},
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let source_book_id: i64 =
        sqlx::query_scalar("SELECT id FROM books WHERE scan_key = 'source.epub'")
            .fetch_one(&pool)
            .await
            .unwrap();

    let author_source_id = author_id_by_name(&pool, "John Smith").await;
    let author_canonical_id = author_id_by_name(&pool, "J. Smith").await;
    let series_source_id = series_id_by_name(&pool, "The Wheel of Time").await;
    let series_canonical_id = series_id_by_name(&pool, "Wheel of Time").await;
    let tag_source_id = tag_id_by_name(&pool, "Sci-Fi").await;
    let tag_canonical_id = tag_id_by_name(&pool, "Science Fiction").await;

    apply_merge_authors(&pool, &[author_source_id], author_canonical_id, None, None)
        .await
        .unwrap();
    apply_merge_series(&pool, &[series_source_id], series_canonical_id, None, None)
        .await
        .unwrap();
    apply_merge_tags(&pool, &[tag_source_id], tag_canonical_id, None, None)
        .await
        .unwrap();

    // A second reindex of the source book: its file on disk never changed,
    // so the parser still reports the names the merges just absorbed.
    let reparsed = [indexed(
        "source.epub",
        Some("Source Book"),
        &["John Smith"],
        &["Sci-Fi"],
        Some(("The Wheel of Time", "1")),
        None,
    )];
    let mut tx = pool.begin().await.unwrap();
    // #1985: the caller now pre-resolves the batch's alias map itself,
    // reflecting the merges just applied above, instead of `sync_changed`
    // resolving it per book internally.
    let alias_maps = collect_entity_alias_maps(&mut tx, &reparsed, &[])
        .await
        .unwrap();
    sync_changed(&mut tx, library_id, "/lib", &reparsed, &alias_maps, |_| {})
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(
        taxonomy_row_missing(&pool, "authors", "John Smith").await,
        "AC2: the merged-away author must not be recreated"
    );
    assert!(
        taxonomy_row_missing(&pool, "series", "The Wheel of Time").await,
        "AC2: the merged-away series must not be recreated"
    );
    assert!(
        taxonomy_row_missing(&pool, "tags", "Sci-Fi").await,
        "AC2: the merged-away tag must not be recreated"
    );

    assert!(
        is_linked(
            &pool,
            "books_authors_link",
            "author",
            source_book_id,
            author_canonical_id
        )
        .await,
        "AC1: reindexed book resolves to the canonical author"
    );
    assert!(
        is_linked(
            &pool,
            "books_series_link",
            "series",
            source_book_id,
            series_canonical_id
        )
        .await,
        "AC1: reindexed book resolves to the canonical series"
    );
    assert!(
        is_linked(
            &pool,
            "books_tags_link",
            "tag",
            source_book_id,
            tag_canonical_id
        )
        .await,
        "AC1: reindexed book resolves to the canonical tag"
    );
}

/// AC3: with no alias recorded, `sync_changed` behaves exactly as before —
/// the reindexed name resolves to its own (unmerged) row, not some other
/// entity's.
#[tokio::test]
async fn sync_changed_reindex_leaves_unmerged_entities_unaffected() {
    let _covers = CoversTempDir::new("resurrection_guard_unaffected");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;

    let book = indexed(
        "plain.epub",
        Some("Plain Book"),
        &["Ordinary Author"],
        &["Ordinary Tag"],
        Some(("Ordinary Series", "1")),
        None,
    );
    let mut tx = pool.begin().await.unwrap();
    sync_new(
        &mut tx,
        library_id,
        "/lib",
        &[book],
        &[],
        &EntityAliasMaps::default(),
        |_| {},
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE scan_key = 'plain.epub'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let author_id = author_id_by_name(&pool, "Ordinary Author").await;
    let series_id = series_id_by_name(&pool, "Ordinary Series").await;
    let tag_id = tag_id_by_name(&pool, "Ordinary Tag").await;

    let reparsed = indexed(
        "plain.epub",
        Some("Plain Book"),
        &["Ordinary Author"],
        &["Ordinary Tag"],
        Some(("Ordinary Series", "1")),
        None,
    );
    let mut tx = pool.begin().await.unwrap();
    sync_changed(
        &mut tx,
        library_id,
        "/lib",
        &[reparsed],
        &EntityAliasMaps::default(),
        |_| {},
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM authors").await,
        1,
        "no extra author row minted"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM series").await,
        1,
        "no extra series row minted"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM tags").await,
        1,
        "no extra tag row minted"
    );
    assert!(is_linked(&pool, "books_authors_link", "author", book_id, author_id).await);
    assert!(is_linked(&pool, "books_series_link", "series", book_id, series_id).await);
    assert!(is_linked(&pool, "books_tags_link", "tag", book_id, tag_id).await);
}

/// Counts `tracing` events sqlx emits (target `"sqlx::query"`, one per
/// executed statement) whose `db.statement` field mentions `entity_aliases` —
/// i.e. a `resolve_entity_aliases` lookup specifically, not every query the
/// sync writes. Mirrors the `QueryCounter` pattern in
/// `db/src/epub_rewrite/tests.rs`, narrowed to the one table this test cares
/// about so the assertion isn't swamped by the per-book `books`/link-table
/// writes a reindex batch also issues.
///
/// Installed as the **global** default (not `tracing::subscriber::set_default`,
/// which is thread-local): `sqlx-sqlite` runs every statement — including
/// each one this counts — on a dedicated per-connection worker thread (see
/// `sqlx_sqlite::connection::worker`), so a thread-local override on the
/// test's own async task never sees them. Measured, not assumed: the scoped
/// form counts 0 here and the assertion below then passes vacuously, which
/// is the failure mode the lower bound exists to catch.
///
/// A global default is process-wide and permanent for the rest of the test
/// binary. That is safe because this is the only `set_global_default` in the
/// crate, so nothing races it for the slot — and the workspace runs on
/// nextest, which gives each test its own process anyway. It is still why
/// the upper bound is loose rather than exact: a concurrently-running
/// unrelated test's own `entity_aliases` query could add a stray hit.
struct EntityAliasQueryCounter(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl tracing::Subscriber for EntityAliasQueryCounter {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "sqlx::query"
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct MentionsEntityAliases(bool);
        impl tracing::field::Visit for MentionsEntityAliases {
            fn record_debug(
                &mut self,
                _field: &tracing::field::Field,
                _value: &dyn std::fmt::Debug,
            ) {
            }
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if field.name().ends_with("statement") && value.contains("entity_aliases") {
                    self.0 = true;
                }
            }
        }
        let mut visitor = MentionsEntityAliases(false);
        event.record(&mut visitor);
        if visitor.0 {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

/// #1985 AC3: a multi-book reindex batch issues an `entity_aliases` lookup
/// count bounded by the touched `CleanupKind`s (author, series, tag — a
/// small constant), not by book count. The old per-book shape issued
/// `3 * BOOK_COUNT` such queries (`insert_author_links`/`insert_tag_links`/
/// `resolve_or_insert_series` each called `resolve_entity_aliases` once per
/// book); the batched shape issues 3 total regardless of `BOOK_COUNT`. The
/// upper bound checks `< BOOK_COUNT` rather than `== 3` — see
/// `EntityAliasQueryCounter`'s doc for why an exact count isn't safe to
/// assert against a process-wide subscriber — but `BOOK_COUNT` is picked
/// large enough that the old shape would fail this bound by a wide margin
/// while the new shape passes with room to spare. The lower bound is what
/// keeps that upper bound honest: a subscriber that never sees an event
/// counts 0, and `0 < BOOK_COUNT` would pass no matter how the batching
/// regressed.
#[tokio::test]
async fn sync_books_issues_one_entity_alias_query_per_kind_not_per_book() {
    let _covers = CoversTempDir::new("sync_books_alias_query_count");
    let pool = init_db("sqlite::memory:").await.unwrap();

    const BOOK_COUNT: usize = 20;
    let new_books: Vec<IndexedBook> = (0..BOOK_COUNT)
        .map(|i| {
            let filename = format!("book-{i}.epub");
            let title = format!("Book {i}");
            let author = format!("Author {i}");
            let tag = format!("Tag {i}");
            let series = format!("Series {i}");
            indexed(
                &filename,
                Some(&title),
                &[&author],
                &[&tag],
                Some((&series, "1")),
                None,
            )
        })
        .collect();

    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // Best-effort: a prior test in this binary may have already claimed the
    // global default (it can only be set once per process). If so, this
    // counter simply stays at 0 and the loose bound below still holds —
    // it just stops being a meaningful check for this particular run.
    let _ = tracing::subscriber::set_global_default(EntityAliasQueryCounter(count.clone()));
    // Interest in the `sqlx::query` callsite is cached process-wide the
    // first time it fires and is *not* refreshed by installing a new
    // default alone — `init_db`'s migrations above already fired it under
    // whatever (no-op) dispatcher preceded this one, which would otherwise
    // leave it permanently cached "never interested".
    tracing::callsite::rebuild_interest_cache();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let queries = count.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        queries > 0,
        "counted no entity_aliases queries at all — the subscriber never saw \
         sqlx's events, so the bound below would pass vacuously however the \
         batching regressed"
    );
    assert!(
        queries < BOOK_COUNT,
        "expected an entity_aliases query count bounded by the touched \
         CleanupKinds (author, series, tag — 3, not {BOOK_COUNT}), got \
         {queries} for a {BOOK_COUNT}-book batch"
    );
}
