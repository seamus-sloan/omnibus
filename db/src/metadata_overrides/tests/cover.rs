//! The cover half of the override layer: `clear_cover_override`'s no-op,
//! preserve, and delete-the-row paths, and `write_override_cover`'s typed
//! filesystem errors.

use omnibus_shared::MetadataOverrides;

use crate::pool::init_db;
use crate::test_support::{CoversTempDir, EnvVarGuard};

use super::super::*;

#[tokio::test]
async fn clear_cover_override_is_noop_when_no_override_row_exists() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    // No prior `upsert_metadata_overrides` call for this uuid — must not
    // error or fabricate a row.
    clear_cover_override(&pool, "no-such-uuid", 1)
        .await
        .unwrap();
    assert!(get_metadata_overrides(&pool, "no-such-uuid")
        .await
        .unwrap()
        .is_none());
}
#[tokio::test]
async fn clear_cover_override_is_noop_when_cover_override_not_set() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let ov = MetadataOverrides {
        title: Some("Text Only".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, "text-only-uuid", &ov, false, user_id)
        .await
        .unwrap();

    clear_cover_override(&pool, "text-only-uuid", user_id)
        .await
        .unwrap();

    let (loaded, has_cover) = get_metadata_overrides(&pool, "text-only-uuid")
        .await
        .unwrap()
        .expect("text override row must survive a no-op cover clear");
    assert_eq!(loaded.title.as_deref(), Some("Text Only"));
    assert!(!has_cover);
}
#[tokio::test]
async fn clear_cover_override_preserves_text_overrides_when_present() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let ov = MetadataOverrides {
        title: Some("Kept Title".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, "mixed-uuid", &ov, true, user_id)
        .await
        .unwrap();

    clear_cover_override(&pool, "mixed-uuid", user_id)
        .await
        .unwrap();

    let (loaded, has_cover) = get_metadata_overrides(&pool, "mixed-uuid")
        .await
        .unwrap()
        .expect("text override must survive a cover-only clear");
    assert_eq!(loaded.title.as_deref(), Some("Kept Title"));
    assert!(!has_cover, "cover flag must be cleared");
}
#[tokio::test]
async fn clear_cover_override_deletes_row_when_no_overrides_remain() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    // A cover-only override: no text fields set.
    upsert_metadata_overrides(
        &pool,
        "cover-only-uuid",
        &MetadataOverrides::default(),
        true,
        user_id,
    )
    .await
    .unwrap();

    clear_cover_override(&pool, "cover-only-uuid", user_id)
        .await
        .unwrap();

    assert!(
        get_metadata_overrides(&pool, "cover-only-uuid")
            .await
            .unwrap()
            .is_none(),
        "an override row with nothing left active must be deleted, not left empty"
    );
}
/// #1395: mirrors `delete_metadata_overrides_removes_stale_export_epub_cache`
/// — clearing a cover-only override deletes the whole row (nothing left
/// active), so the stale export cache must go too.
#[tokio::test]
async fn clear_cover_override_removes_stale_export_epub_cache_when_nothing_remains() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = crate::test_support::seed_synced_ebook(&pool, "b.epub", "T", "A").await;
    let id = crate::resolve_book_id_by_uuid(&pool, &uuid)
        .await
        .unwrap()
        .unwrap();

    // A cover-only override: clearing it empties the row entirely.
    upsert_metadata_overrides(&pool, &uuid, &MetadataOverrides::default(), true, user_id)
        .await
        .unwrap();

    let cache_path = crate::epub_rewrite::export_epub_path(id);
    std::fs::write(&cache_path, b"stale rewritten epub").unwrap();

    clear_cover_override(&pool, &uuid, user_id).await.unwrap();

    assert!(
        !cache_path.exists(),
        "clearing the last active (cover) override must delete the export cache file"
    );
}

/// Counterpart: a cover clear that leaves a text override in place must NOT
/// delete the export cache, since the book still needs a rewrite (just
/// without the cover swap) — the `last_modified` bump already forces
/// `rewritten_epub_path` to regenerate rather than serve a torn-down file.
#[tokio::test]
async fn clear_cover_override_keeps_export_epub_cache_when_text_overrides_remain() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = crate::test_support::seed_synced_ebook(&pool, "b.epub", "T", "A").await;
    let id = crate::resolve_book_id_by_uuid(&pool, &uuid)
        .await
        .unwrap()
        .unwrap();

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            title: Some("Kept Title".into()),
            ..Default::default()
        },
        true,
        user_id,
    )
    .await
    .unwrap();

    let cache_path = crate::epub_rewrite::export_epub_path(id);
    std::fs::write(&cache_path, b"stale rewritten epub").unwrap();

    clear_cover_override(&pool, &uuid, user_id).await.unwrap();

    assert!(
        cache_path.exists(),
        "a text override still active means the export cache isn't dead weight yet"
    );
}

#[tokio::test]
async fn clear_cover_override_returns_serialization_error_for_corrupt_blob() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query(
        "INSERT INTO metadata_overrides (book_uuid, overrides, has_cover_override) \
         VALUES (?, ?, 1)",
    )
    .bind("corrupt-cover-uuid")
    .bind("{ not valid json")
    .execute(&pool)
    .await
    .unwrap();

    let err = clear_cover_override(&pool, "corrupt-cover-uuid", 1)
        .await
        .expect_err("corrupt overrides JSON must not decode");
    assert!(
        matches!(err, MetadataOverridesError::Serialization(_)),
        "got {err:?}"
    );
}

/// Happy path for the filesystem-only cover write helper: the bytes land at
/// `<covers_dir>/override-<uuid>.<ext>` with the extension derived from the
/// declared MIME type.
#[test]
fn write_override_cover_writes_bytes_to_the_expected_path() {
    let covers = CoversTempDir::new("write_cover_ok");

    write_override_cover("happy-uuid", "image/png", b"OVERRIDE-BYTES").unwrap();

    let written = covers.path.join("override-happy-uuid.png");
    assert_eq!(std::fs::read(written).unwrap(), b"OVERRIDE-BYTES");
}

/// `create_dir_all` fails when a regular file already occupies the covers
/// dir path, deterministically forcing the `std::io::Error` branch. The
/// failure must surface as the module's typed `MetadataOverridesError::Io`,
/// not a raw `std::io::Error`.
#[test]
fn write_override_cover_returns_typed_error_when_covers_dir_is_unwritable() {
    let covers = CoversTempDir::new("write_cover_fail");
    std::fs::write(&covers.path, b"not a directory").unwrap();

    let err = write_override_cover("some-uuid", "image/png", b"bytes")
        .expect_err("create_dir_all must fail when the covers dir path is a regular file");
    assert!(matches!(err, MetadataOverridesError::Io(_)), "got {err:?}");
}

/// A non-`NotFound` failure in the stale-extension cleanup loop (here, a
/// directory occupying the path a prior override cover would live at, so
/// `remove_file` fails with `IsADirectory` rather than `NotFound`) must
/// propagate as `MetadataOverridesError::Io` instead of being swallowed —
/// silently leaving the stale entry behind would let `find_override_cover_file`
/// keep probing over it ahead of the freshly written cover.
#[test]
fn write_override_cover_returns_typed_error_when_stale_cleanup_hits_a_directory() {
    let covers = CoversTempDir::new("write_cover_cleanup_fail");
    std::fs::create_dir_all(&covers.path).unwrap();
    // `Jpeg` is first in `PROBE_ORDER`, so this is the first cleanup target.
    std::fs::create_dir(covers.path.join("override-clash-uuid.jpg")).unwrap();

    let err = write_override_cover("clash-uuid", "image/png", b"bytes")
        .expect_err("remove_file on a directory must fail instead of silently leaving it behind");
    assert!(matches!(err, MetadataOverridesError::Io(_)), "got {err:?}");
}

/// A WebP upload must land on disk as `override-<uuid>.jpg`, not
/// `override-<uuid>.webp` — the Kobo cover route serves the stored file
/// verbatim and its firmware renders no cover for a WebP.
#[test]
fn write_override_cover_stores_a_webp_upload_as_jpeg() {
    let covers = CoversTempDir::new("write_cover_webp");

    let rgba = image::RgbaImage::from_pixel(40, 60, image::Rgba([10, 20, 30, 255]));
    let webp = webp::Encoder::from_rgba(rgba.as_raw(), 40, 60)
        .encode(80.0)
        .to_vec();
    write_override_cover("webp-uuid", "image/webp", &webp).unwrap();

    assert!(
        !covers.path.join("override-webp-uuid.webp").exists(),
        "the WebP extension must not survive the write"
    );
    let written = std::fs::read(covers.path.join("override-webp-uuid.jpg")).unwrap();
    assert_eq!(
        image::guess_format(&written).unwrap(),
        image::ImageFormat::Jpeg
    );
}

/// Re-uploading over an existing override must not leave the previous
/// extension behind: `find_override_cover_file` probes JPEG before WebP, so
/// a stale `.jpg` would shadow a later `.png` entirely.
#[test]
fn write_override_cover_replaces_a_prior_jpeg_when_a_png_is_uploaded() {
    let covers = CoversTempDir::new("write_cover_replace");

    let rgba = image::RgbaImage::from_pixel(40, 60, image::Rgba([1, 2, 3, 255]));
    let webp = webp::Encoder::from_rgba(rgba.as_raw(), 40, 60)
        .encode(80.0)
        .to_vec();
    write_override_cover("swap-uuid", "image/webp", &webp).unwrap();
    assert!(covers.path.join("override-swap-uuid.jpg").exists());

    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(40, 60, image::Rgb([9, 9, 9])))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    write_override_cover("swap-uuid", "image/png", &png).unwrap();

    assert!(
        !covers.path.join("override-swap-uuid.jpg").exists(),
        "the transcoded JPEG from the first upload must be cleaned up"
    );
    assert!(covers.path.join("override-swap-uuid.png").exists());
}
