//! Unit tests for cover resolution: last-modified fallback, missing-file
//! handling, and override-vs-original cover selection.

use omnibus_shared::MetadataOverrides;

use super::*;
use crate::books::list_books;
use crate::metadata_overrides::{upsert_metadata_overrides, write_override_cover};
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};

#[tokio::test]
async fn get_last_modified_epoch_falls_back_to_now_when_column_null() {
    // `books.last_modified` is nullable after 0038's in-place conversion; a bare
    // `i64` decode of a NULL would error and 500 `/api/thumbs/*`. The COALESCE
    // keeps it serving (and regenerates the stale thumb).
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, last_modified) \
         VALUES ('bk', 'b', 1, '/lib/bk', 'Book', NULL) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let got = get_last_modified_epoch(&pool, id).await.unwrap();
    assert!(matches!(got, Some(e) if e > 1_700_000_000), "got {got:?}");
    // A missing book still yields None rather than a fabricated epoch.
    assert_eq!(get_last_modified_epoch(&pool, 99_999).await.unwrap(), None);
}

#[tokio::test]
async fn cover_returns_none_when_file_missing() {
    let _covers = CoversTempDir::new("missing");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("A"),
            &["A"],
            &[],
            None,
            Some(("image/jpeg", b"BYTES")),
        )],
    )
    .await
    .unwrap();
    let books = list_books(&pool, "/lib").await.unwrap();
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(books[0].id)
        .fetch_one(&pool)
        .await
        .unwrap();
    // Remove the file out from under the DB — get_cover should report
    // None, not error.
    let _ = std::fs::remove_file(cover_path_for(&uuid, "jpg"));
    assert!(get_cover(&pool, books[0].id).await.unwrap().is_none());
}
/// When a `metadata_overrides` row sets `has_cover_override = 1` and an
/// `override-<uuid>.<ext>` file exists on disk, `get_cover` returns the
/// override bytes — not the scanned cover. Single-query form must
/// preserve this precedence.
#[tokio::test]
async fn cover_returns_override_when_flag_set() {
    let _covers = CoversTempDir::new("override_set");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("A"),
            &["A"],
            &[],
            None,
            Some(("image/jpeg", b"ORIGINAL")),
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let uuid = books[0].unique_identifier.clone().unwrap();

    // Mark cover-override + drop the override file on disk.
    write_override_cover(&uuid, "image/png", b"OVERRIDE").unwrap();
    upsert_metadata_overrides(&pool, &uuid, &MetadataOverrides::default(), true, user_id)
        .await
        .unwrap();

    let cover = get_cover(&pool, books[0].id).await.unwrap();
    assert_eq!(cover, Some(("image/png".into(), b"OVERRIDE".to_vec())));
}
/// With no `metadata_overrides` row, `get_cover` falls through to the
/// scanned `<uuid>.<ext>` cover. The LEFT JOIN must not filter the book
/// out when no override row exists.
#[tokio::test]
async fn cover_returns_original_when_no_override_row() {
    let _covers = CoversTempDir::new("override_absent");
    let pool = init_db("sqlite::memory:").await.unwrap();

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("A"),
            &["A"],
            &[],
            None,
            Some(("image/jpeg", b"ORIGINAL")),
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let cover = get_cover(&pool, books[0].id).await.unwrap();
    assert_eq!(cover, Some(("image/jpeg".into(), b"ORIGINAL".to_vec())));
}
/// A `metadata_overrides` row with `has_cover_override = 0` (text-only
/// edits, no cover swap) must resolve to the scanned cover, not the
/// override path.
#[tokio::test]
async fn cover_returns_original_when_override_flag_unset() {
    let _covers = CoversTempDir::new("override_flag_off");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("A"),
            &["A"],
            &[],
            None,
            Some(("image/jpeg", b"ORIGINAL")),
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let uuid = books[0].unique_identifier.clone().unwrap();

    // Override row exists with text edits but no cover swap.
    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            title: Some("Edited".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let cover = get_cover(&pool, books[0].id).await.unwrap();
    assert_eq!(cover, Some(("image/jpeg".into(), b"ORIGINAL".to_vec())));
}
/// `get_cover` for a non-existent book id returns `Ok(None)` (not an
/// error). The LEFT JOIN must not change this contract.
#[tokio::test]
async fn cover_returns_none_for_missing_book_id() {
    let _covers = CoversTempDir::new("missing_book");
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert!(get_cover(&pool, 999_999).await.unwrap().is_none());
}

/// A cover stored under `<uuid>.png` reports `image/png`, not the hardcoded
/// `image/jpeg` a caller might otherwise assume (#1772).
#[test]
fn cover_mime_hint_reflects_a_png_cover_on_disk() {
    let _covers = CoversTempDir::new("mime_hint_png");
    std::fs::create_dir_all(covers_dir()).unwrap();
    std::fs::write(cover_path_for("png-book", "png"), b"PNGBYTES").unwrap();
    assert_eq!(cover_mime_hint("png-book", false), "image/png");
}

/// With `has_cover_override` set, the override file's format wins even when
/// a differently-formatted original also exists on disk — matching
/// [`get_cover`]'s own precedence.
#[test]
fn cover_mime_hint_prefers_the_override_file_when_flagged() {
    let _covers = CoversTempDir::new("mime_hint_override");
    std::fs::create_dir_all(covers_dir()).unwrap();
    std::fs::write(cover_path_for("ov-book", "jpg"), b"ORIGINAL").unwrap();
    std::fs::write(covers_dir().join("override-ov-book.webp"), b"OVERRIDEBYTES").unwrap();
    assert_eq!(cover_mime_hint("ov-book", true), "image/webp");
}

/// No cover file under any known extension falls back to `image/jpeg` — the
/// literal every OPDS entry advertised before this lookup existed, and the
/// byte-serving endpoint 404s regardless so the advertised type is moot.
#[test]
fn cover_mime_hint_falls_back_to_jpeg_when_no_file_exists() {
    let _covers = CoversTempDir::new("mime_hint_missing");
    assert_eq!(cover_mime_hint("no-such-book", false), "image/jpeg");
}

/// A minimal but valid 1x1 GIF87a: header + logical-screen descriptor + a
/// 2-colour global table + one image descriptor and a single-pixel LZW frame.
/// Enough for `image` to sniff the format and decode a first frame.
const GIF_1X1: &[u8] = &[
    0x47, 0x49, 0x46, 0x38, 0x37, 0x61, // "GIF87a"
    0x01, 0x00, 0x01, 0x00, // 1x1 logical screen
    0x80, 0x00, 0x00, // GCT flag, 2 colours, no bg/aspect
    0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, // palette: black, white
    0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, // image descriptor
    0x02, 0x02, 0x44, 0x01, 0x00, // LZW min code size + 1-pixel data block
    0x3B, // trailer
];

/// A cover whose bytes are really a GIF must be written as `<uuid>.gif` even
/// when the caller hands us `image/jpeg` — the extension is sniffed from the
/// bytes, not trusted from the mime. This is the root cause of covers
/// landing as `<uuid>.jpg` and later failing to decode.
#[tokio::test]
async fn write_cover_file_stores_gif_when_bytes_are_gif_despite_jpeg_mime() {
    let _covers = CoversTempDir::new("gif_extension");
    let uuid = "gif-book";

    write_cover_file(uuid, "image/jpeg", GIF_1X1).unwrap();

    // The `.gif` path exists with the GIF bytes; no `.jpg` was written.
    assert_eq!(std::fs::read(cover_path_for(uuid, "gif")).unwrap(), GIF_1X1);
    assert!(std::fs::read(cover_path_for(uuid, "jpg")).is_err());
}

/// The `gif` codec is compiled into the `image` crate, so GIF cover bytes
/// decode via `load_from_memory` instead of erroring with "The image format
/// Gif is not supported" — the fatal failure reported in #828.
#[test]
fn gif_cover_bytes_decode_via_load_from_memory() {
    let img =
        image::load_from_memory(GIF_1X1).expect("GIF must decode once the gif codec is enabled");
    assert_eq!((img.width(), img.height()), (1, 1));
}

#[tokio::test]
async fn get_last_modified_epoch_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = get_last_modified_epoch(&pool, 1).await.unwrap_err();
    assert!(matches!(err, CoversError::Db(_)));
}

/// Encode a solid-colour raster of the given size, for the normalization
/// tests below. WebP is decode-only in our `image` feature set, so a real
/// WebP fixture has to come from bytes rather than an encoder — see
/// [`webp_fixture`].
fn raster(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    }));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), format)
        .unwrap();
    out
}

/// A real lossy-WebP fixture, encoded with the `webp` crate the thumbnail
/// pipeline already depends on.
fn webp_fixture(width: u32, height: u32) -> Vec<u8> {
    let rgba = image::RgbaImage::from_fn(width, height, |x, y| {
        image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255])
    });
    webp::Encoder::from_rgba(rgba.as_raw(), width, height)
        .encode(80.0)
        .to_vec()
}

#[test]
fn normalize_override_cover_transcodes_webp_to_jpeg() {
    // The bug this exists for: Kobo renders no cover at all for a WebP, and
    // the route it fetches is literally `image.jpg`.
    let (mime, bytes) = normalize_override_cover("image/webp", &webp_fixture(400, 600));

    assert_eq!(mime, "image/jpeg");
    assert_eq!(
        image::guess_format(&bytes).unwrap(),
        image::ImageFormat::Jpeg
    );
}

#[test]
fn normalize_override_cover_transcodes_webp_mislabelled_as_jpeg() {
    // The format is sniffed from the bytes, not the declared mime (#828) —
    // a WebP arriving as `image/jpeg` is exactly the case that would
    // otherwise reach a Kobo untranscoded.
    let (mime, bytes) = normalize_override_cover("image/jpeg", &webp_fixture(400, 600));

    assert_eq!(mime, "image/jpeg");
    assert_eq!(
        image::guess_format(&bytes).unwrap(),
        image::ImageFormat::Jpeg
    );
}

#[test]
fn normalize_override_cover_leaves_png_untouched() {
    // PNG is confirmed working on-device, so it must not pay a re-encode.
    let png = raster(400, 600, image::ImageFormat::Png);
    let (mime, bytes) = normalize_override_cover("image/png", &png);

    assert_eq!(mime, "image/png");
    assert_eq!(bytes, png, "a correctly-sized PNG must pass through as-is");
}

#[test]
fn normalize_override_cover_leaves_correctly_sized_jpeg_byte_identical() {
    let jpeg = raster(400, 600, image::ImageFormat::Jpeg);
    let (mime, bytes) = normalize_override_cover("image/jpeg", &jpeg);

    assert_eq!(mime, "image/jpeg");
    assert_eq!(
        bytes, jpeg,
        "re-encoding an already-valid JPEG would cost a generation of quality for nothing"
    );
}

#[test]
fn normalize_override_cover_downscales_an_oversized_image_preserving_aspect_ratio() {
    // The reported upload was 4281x5726 (~24 MP) for a tile rendered ~500px
    // tall; 2:3 here so the rounded width is checkable.
    let png = raster(1600, 2400, image::ImageFormat::Png);
    let (mime, bytes) = normalize_override_cover("image/png", &png);

    assert_eq!(mime, "image/png");
    let decoded = image::load_from_memory(&bytes).unwrap();
    assert_eq!(decoded.height(), MAX_OVERRIDE_COVER_HEIGHT);
    assert_eq!(decoded.width(), 800, "2:3 aspect ratio must be preserved");
    assert!(
        bytes.len() < png.len(),
        "downscaled cover should be smaller: {} vs {}",
        bytes.len(),
        png.len()
    );
}

#[test]
fn normalize_override_cover_downscales_and_transcodes_an_oversized_webp() {
    let (mime, bytes) = normalize_override_cover("image/webp", &webp_fixture(1600, 2400));

    assert_eq!(mime, "image/jpeg");
    let decoded = image::load_from_memory(&bytes).unwrap();
    assert_eq!(decoded.height(), MAX_OVERRIDE_COVER_HEIGHT);
}

#[test]
fn normalize_override_cover_leaves_a_short_image_at_its_own_height() {
    // Only oversized covers are touched — upscaling a small cover would
    // invent detail that isn't there.
    let png = raster(200, 300, image::ImageFormat::Png);
    let (_, bytes) = normalize_override_cover("image/png", &png);

    assert_eq!(bytes, png);
}

#[test]
fn normalize_override_cover_leaves_gif_untouched_even_when_oversized() {
    // `image` decodes only a GIF's first frame, so any re-encode here would
    // silently flatten an animation.
    let gif = raster(1600, 2400, image::ImageFormat::Gif);
    let (mime, bytes) = normalize_override_cover("image/gif", &gif);

    assert_eq!(mime, "image/gif");
    assert_eq!(bytes, gif);
}

#[test]
fn normalize_override_cover_passes_through_undecodable_bytes() {
    // SVG and anything else we can't decode must never fail an upload —
    // passing through is the pre-existing behaviour.
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" height="1"/></svg>"#;
    let (mime, bytes) = normalize_override_cover("image/svg+xml", svg);

    assert_eq!(mime, "image/svg+xml");
    assert_eq!(bytes, svg);

    let (mime, bytes) = normalize_override_cover("image/jpeg", b"not an image at all");
    assert_eq!(mime, "image/jpeg");
    assert_eq!(bytes, b"not an image at all");
}
