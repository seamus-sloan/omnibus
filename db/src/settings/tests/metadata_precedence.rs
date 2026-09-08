//! Per-library metadata precedence: the default for an unconfigured
//! path, the round trip creating or updating the `scan_roots` row, the
//! fallbacks for an unparseable or incomplete stored value, and the
//! by-uuid resolution through each book's scan root.

use super::super::*;
use crate::books::list_books;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::indexed;

// Per-library metadata precedence (F5.1, #972)
#[tokio::test]
async fn get_metadata_precedence_returns_default_for_unconfigured_path() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert_eq!(
        get_metadata_precedence(&pool, "/never-scanned")
            .await
            .unwrap(),
        DEFAULT_METADATA_PRECEDENCE.to_vec()
    );
}

#[tokio::test]
async fn set_and_get_metadata_precedence_roundtrips() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let order = vec![
        MetadataSource::ProviderMatch,
        MetadataSource::OmnibusOverrides,
        MetadataSource::EmbeddedTags,
        MetadataSource::OpfSidecar,
        MetadataSource::FolderStructure,
    ];
    set_metadata_precedence(&pool, "/lib", &order)
        .await
        .unwrap();
    assert_eq!(get_metadata_precedence(&pool, "/lib").await.unwrap(), order);
}

#[tokio::test]
async fn set_metadata_precedence_creates_the_scan_root_row_when_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_metadata_precedence(&pool, "/never-scanned", &DEFAULT_METADATA_PRECEDENCE)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scan_roots WHERE path = ?")
        .bind("/never-scanned")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "saving the setting before a scan creates the row");
}

#[tokio::test]
async fn set_metadata_precedence_updates_existing_row_without_creating_a_duplicate() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, ?)")
        .bind("/lib")
        .bind("lib")
        .execute(&pool)
        .await
        .unwrap();
    set_metadata_precedence(
        &pool,
        "/lib",
        &[
            MetadataSource::OmnibusOverrides,
            MetadataSource::EmbeddedTags,
            MetadataSource::FolderStructure,
            MetadataSource::OpfSidecar,
            MetadataSource::ProviderMatch,
        ],
    )
    .await
    .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scan_roots")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "upsert must not duplicate the existing row");
}

#[tokio::test]
async fn get_metadata_precedence_falls_back_to_default_for_unparseable_value() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query(
        "INSERT INTO scan_roots (path, display_name, metadata_precedence) VALUES (?, ?, ?)",
    )
    .bind("/lib")
    .bind("lib")
    .bind("not-json")
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        get_metadata_precedence(&pool, "/lib").await.unwrap(),
        DEFAULT_METADATA_PRECEDENCE.to_vec()
    );
}

#[tokio::test]
async fn get_metadata_precedence_falls_back_to_default_for_valid_json_missing_a_source() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    // Well-formed JSON, but only lists one of the 5 known sources — e.g. a
    // hand-edited row. Must not be treated as authoritative: silently
    // dropping sources from the merge order is worse than falling back.
    sqlx::query(
        "INSERT INTO scan_roots (path, display_name, metadata_precedence) VALUES (?, ?, ?)",
    )
    .bind("/lib")
    .bind("lib")
    .bind(r#"["embedded_tags"]"#)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        get_metadata_precedence(&pool, "/lib").await.unwrap(),
        DEFAULT_METADATA_PRECEDENCE.to_vec()
    );
}

#[tokio::test]
async fn metadata_precedence_by_uuid_resolves_each_books_scan_root() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
    )
    .await
    .unwrap();
    let uuid = list_books(&pool, "/lib").await.unwrap()[0]
        .unique_identifier
        .clone()
        .unwrap();
    set_metadata_precedence(
        &pool,
        "/lib",
        &[
            MetadataSource::OmnibusOverrides,
            MetadataSource::EmbeddedTags,
            MetadataSource::FolderStructure,
            MetadataSource::OpfSidecar,
            MetadataSource::ProviderMatch,
        ],
    )
    .await
    .unwrap();

    let map = metadata_precedence_by_uuid(&pool, std::slice::from_ref(&uuid))
        .await
        .unwrap();
    assert_eq!(map.get(&uuid).unwrap()[0], MetadataSource::OmnibusOverrides);
}

#[tokio::test]
async fn metadata_precedence_by_uuid_omits_unknown_uuids() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let map = metadata_precedence_by_uuid(&pool, &["no-such-uuid".to_string()])
        .await
        .unwrap();
    assert!(map.is_empty());
}
