//! Unit tests for the `thumbs` module — `thumbs_dir` env-var resolution,
//! `thumb_path_for` formatting, `is_stale` mtime comparison (incl. the
//! same-second tie case), `ThumbSize` FromStr roundtrip, `thumb_etag`
//! derivation, `generate_thumbnail`'s lossy WebP output and size budget,
//! `purge_stale_scheme_once` re-encode invalidation, and LRU-on-read
//! `evict_if_over_cap` cap enforcement.

use super::*;
use crate::test_support::EnvVarGuard;

/// A synthetic stand-in for a photographic cover: smooth gradients carrying
/// low-amplitude noise, encoded as PNG. Noise is what separates the two
/// encoders — it defeats the lossless coder's prediction while a lossy
/// quantizer discards most of it — so a flat test image would make the size
/// assertions below pass for the wrong reason. The LCG keeps it deterministic
/// without a `rand` dependency.
fn photographic_png(w: u32, h: u32) -> Vec<u8> {
    use image::{ImageBuffer, Rgb};

    let mut seed: u32 = 0x9e37_79b9;
    let mut next = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((seed >> 16) & 0x0f) as i32 - 8
    };
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(w, h, |x, y| {
        let base = [
            (x * 255 / w) as i32,
            (y * 255 / h) as i32,
            ((x * x + y * y) / 512 % 256) as i32,
        ];
        let n = next();
        Rgb(std::array::from_fn(|i| {
            base[i].saturating_add(n).clamp(0, 255) as u8
        }))
    });

    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    png
}

/// The four-byte chunk id following the `RIFF....WEBP` header: `VP8 ` for a
/// lossy bitstream, `VP8L` for a lossless one, `VP8X` for the extended
/// container an alpha-bearing lossy image uses.
fn webp_fourcc(bytes: &[u8]) -> [u8; 4] {
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WEBP");
    [bytes[12], bytes[13], bytes[14], bytes[15]]
}

/// The old encode path: `image` 0.25's WebP encoder, which is lossless-only.
fn lossless_webp(png: &[u8], size: ThumbSize) -> Vec<u8> {
    use image::imageops::FilterType;

    let (w, h) = size.dimensions();
    let resized = image::load_from_memory(png)
        .unwrap()
        .resize_to_fill(w, h, FilterType::Lanczos3);
    let mut buf = std::io::Cursor::new(Vec::new());
    resized
        .write_to(&mut buf, image::ImageFormat::WebP)
        .unwrap();
    buf.into_inner()
}

#[test]
fn thumbs_dir_defaults_to_dot_thumbs() {
    let _guard = EnvVarGuard::set("OMNIBUS_THUMBS_DIR", None);
    assert_eq!(thumbs_dir(), PathBuf::from("./thumbs"));
}

#[test]
fn thumbs_dir_respects_env_var() {
    let _guard = EnvVarGuard::set("OMNIBUS_THUMBS_DIR", Some("/tmp/omnibus-test-thumbs"));
    assert_eq!(thumbs_dir(), PathBuf::from("/tmp/omnibus-test-thumbs"));
}

#[test]
fn thumb_path_for_format() {
    let _guard = EnvVarGuard::set("OMNIBUS_THUMBS_DIR", None);
    let path = thumb_path_for(42, ThumbSize::Md);
    assert_eq!(path, PathBuf::from("./thumbs/42_md.webp"));
}

#[test]
fn is_stale_returns_true_when_file_missing() {
    let _guard = EnvVarGuard::set("OMNIBUS_THUMBS_DIR", None);
    assert!(is_stale(999999, ThumbSize::Sm, 0));
}

#[test]
fn is_stale_returns_false_when_mtime_is_newer() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));
    std::fs::write(tmp.path().join("1_sm.webp"), b"x").unwrap();
    let mtime = std::fs::metadata(tmp.path().join("1_sm.webp"))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(!is_stale(1, ThumbSize::Sm, mtime - 1));
}

#[test]
fn is_stale_returns_true_when_mtime_ties_last_modified() {
    // Regression for #832 item 2: a thumb regenerated in the same
    // wall-clock second as the triggering cover rewrite must not be
    // mistaken for fresh, or it never regenerates.
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));
    std::fs::write(tmp.path().join("3_sm.webp"), b"x").unwrap();
    let mtime = std::fs::metadata(tmp.path().join("3_sm.webp"))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(is_stale(3, ThumbSize::Sm, mtime));
}

#[test]
fn is_stale_returns_true_when_mtime_is_older() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));
    std::fs::write(tmp.path().join("2_md.webp"), b"x").unwrap();
    let mtime = std::fs::metadata(tmp.path().join("2_md.webp"))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(is_stale(2, ThumbSize::Md, mtime + 1));
}

#[test]
fn thumb_etag_is_stable_for_identical_inputs() {
    assert_eq!(
        thumb_etag(1, ThumbSize::Md, 100),
        thumb_etag(1, ThumbSize::Md, 100)
    );
}

#[test]
fn thumb_etag_differs_when_book_id_changes() {
    assert_ne!(
        thumb_etag(1, ThumbSize::Md, 100),
        thumb_etag(2, ThumbSize::Md, 100)
    );
}

#[test]
fn thumb_etag_differs_when_size_changes() {
    assert_ne!(
        thumb_etag(1, ThumbSize::Sm, 100),
        thumb_etag(1, ThumbSize::Md, 100)
    );
}

#[test]
fn thumb_etag_differs_when_last_modified_epoch_changes() {
    // #1751 AC3: a thumb regenerated because its book's
    // `last_modified_epoch` moved must produce a different ETag.
    assert_ne!(
        thumb_etag(1, ThumbSize::Md, 100),
        thumb_etag(1, ThumbSize::Md, 200)
    );
}

#[test]
fn thumb_etag_differs_when_the_encoder_version_component_changes() {
    // #1751 AC4: folding a version constant into the hash means a future
    // encoder/format change (a bumped `THUMB_ENCODER_VERSION`) produces a
    // different ETag even when `(book_id, size, last_modified_epoch)` is
    // unchanged. Exercised through the version-parameterized helper since
    // the real constant is fixed at compile time.
    assert_ne!(
        thumb_etag_versioned(1, ThumbSize::Md, 100, 1),
        thumb_etag_versioned(1, ThumbSize::Md, 100, 2)
    );
}

#[test]
fn thumb_size_from_str_roundtrip() {
    assert_eq!("sm".parse::<ThumbSize>(), Ok(ThumbSize::Sm));
    assert_eq!("md".parse::<ThumbSize>(), Ok(ThumbSize::Md));
    assert_eq!("lg".parse::<ThumbSize>(), Ok(ThumbSize::Lg));
    assert_eq!("xl".parse::<ThumbSize>(), Err(()));
}

#[test]
fn generate_thumbnail_produces_valid_webp() {
    // Create a synthetic 100×150 white PNG in memory (matches the 2:3
    // cover aspect ratio so resize_to_fill doesn't crop the pixels).
    use image::{ImageBuffer, Rgba};
    let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_fn(100, 150, |_, _| Rgba([255u8, 255, 255, 255]));
    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));
    let bytes_written = generate_thumbnail(10, ThumbSize::Sm, &png_bytes).unwrap();

    assert!(bytes_written > 0);
    let out = std::fs::read(tmp.path().join("10_sm.webp")).unwrap();
    // RIFF....WEBP magic
    assert_eq!(&out[0..4], b"RIFF");
    assert_eq!(&out[8..12], b"WEBP");
}

#[test]
fn generate_thumbnail_writes_a_lossy_bitstream_rather_than_a_lossless_one() {
    let png = photographic_png(800, 1200);
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));

    generate_thumbnail(20, ThumbSize::Md, &png).unwrap();

    let out = std::fs::read(tmp.path().join("20_md.webp")).unwrap();
    assert_eq!(
        &webp_fourcc(&out),
        b"VP8 ",
        "an opaque cover must encode as a plain lossy VP8 chunk, not VP8L"
    );
}

#[test]
fn generate_thumbnail_keeps_a_photographic_cover_inside_the_per_size_byte_budget() {
    let png = photographic_png(800, 1200);
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));

    let md = generate_thumbnail(21, ThumbSize::Md, &png).unwrap();
    let lg = generate_thumbnail(21, ThumbSize::Lg, &png).unwrap();

    assert!(md < 30_000, "md thumb was {md} bytes");
    assert!(lg < 80_000, "lg thumb was {lg} bytes");
}

#[test]
fn generate_thumbnail_is_far_smaller_than_the_lossless_encoder_at_every_size() {
    // The regression this guards is a cache several times the size of the
    // covers it thumbnails; the lossless encoder is the baseline that caused
    // it, so the ratio — not an absolute byte count — is the real contract.
    let png = photographic_png(800, 1200);
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));

    for size in ThumbSize::all() {
        let lossy = generate_thumbnail(22, size, &png).unwrap();
        let lossless = lossless_webp(&png, size).len();
        assert!(
            lossy * 3 < lossless,
            "{size}: lossy {lossy} bytes vs lossless {lossless} bytes"
        );
    }
}

#[test]
fn generate_thumbnail_writes_exact_dimensions_for_every_size() {
    // `resize_to_fill`'s contract, re-asserted through the new encoder: the
    // frontend renders these with fixed width/height attributes, so anything
    // but an exact match stretches covers.
    let png = photographic_png(800, 1000);
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));

    for size in ThumbSize::all() {
        generate_thumbnail(23, size, &png).unwrap();
        let bytes = std::fs::read(tmp.path().join(format!("23_{size}.webp"))).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!(
            (decoded.width(), decoded.height()),
            size.dimensions(),
            "{size} thumb must be exactly its declared dimensions"
        );
    }
}

#[test]
fn generate_thumbnail_preserves_alpha_for_a_cover_with_transparency() {
    use image::{ImageBuffer, Rgba};

    let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_fn(200, 300, |x, _| Rgba([10, 200, 90, (x % 256) as u8]));
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));
    generate_thumbnail(24, ThumbSize::Sm, &png).unwrap();

    let out = std::fs::read(tmp.path().join("24_sm.webp")).unwrap();
    assert_eq!(
        &webp_fourcc(&out),
        b"VP8X",
        "an alpha-bearing cover must use the extended container that carries an ALPH chunk"
    );
}

#[test]
fn has_transparent_pixel_is_false_when_every_pixel_is_fully_opaque() {
    // The decision `encode_lossy_webp` branches on. Asserted directly because
    // the encoded bytes cannot distinguish the two paths — libwebp drops a
    // constant alpha plane itself — so an end-to-end assertion would pass
    // whether or not the RGBA path was correctly skipped.
    let opaque: Vec<u8> = (0..64u8).flat_map(|i| [i, i, i, 255]).collect();
    assert!(!has_transparent_pixel(&opaque));
}

#[test]
fn has_transparent_pixel_is_true_when_a_single_pixel_is_not_fully_opaque() {
    let mut buf: Vec<u8> = (0..64u8).flat_map(|i| [i, i, i, 255]).collect();
    // Last pixel's alpha only — the scan must not stop early on opaque runs.
    let last_alpha = buf.len() - 1;
    buf[last_alpha] = 254;
    assert!(has_transparent_pixel(&buf));
}

#[test]
fn generate_thumbnail_writes_a_plain_lossy_chunk_for_an_rgba_cover_that_is_fully_opaque() {
    use image::{ImageBuffer, Rgba};

    // The pixel format carries an alpha channel but every pixel sets it to
    // 255 — the common shape for a cover that merely happened to be saved as
    // RGBA. This pins the observable contract (no ALPH chunk); the branch that
    // avoids the wasted RGBA conversion is pinned by the two tests above.
    let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_fn(200, 300, |x, y| {
        Rgba([(x % 256) as u8, (y % 256) as u8, 40, 255])
    });
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));
    generate_thumbnail(25, ThumbSize::Sm, &png).unwrap();

    let out = std::fs::read(tmp.path().join("25_sm.webp")).unwrap();
    assert_eq!(
        &webp_fourcc(&out),
        b"VP8 ",
        "a fully-opaque RGBA cover must encode without an alpha plane"
    );
}

// ---------- one-time re-encode invalidation ----------

#[test]
fn purge_stale_scheme_once_removes_thumbnails_left_by_a_previous_scheme() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));
    std::fs::write(tmp.path().join("1_sm.webp"), b"lossless-era bytes").unwrap();
    std::fs::write(tmp.path().join(format!("{SCHEME_SENTINEL_PREFIX}1")), b"\n").unwrap();

    purge_stale_scheme_once();

    assert!(
        !tmp.path().join("1_sm.webp").exists(),
        "a thumb from the previous encoder must not be served indefinitely"
    );
    assert!(
        !tmp.path()
            .join(format!("{SCHEME_SENTINEL_PREFIX}1"))
            .exists(),
        "the previous scheme's sentinel must be swept with its thumbnails"
    );
    assert!(
        tmp.path().join(scheme_sentinel_name()).exists(),
        "the current scheme's sentinel must be written"
    );
}

#[test]
fn purge_stale_scheme_once_leaves_thumbnails_alone_when_the_current_sentinel_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));
    std::fs::write(tmp.path().join(scheme_sentinel_name()), b"\n").unwrap();
    std::fs::write(tmp.path().join("2_md.webp"), b"current-scheme bytes").unwrap();

    purge_stale_scheme_once();

    assert!(
        tmp.path().join("2_md.webp").exists(),
        "the sweep must run once per encoder version, not on every boot"
    );
}

#[test]
fn purge_stale_scheme_once_does_not_create_the_cache_dir_when_it_is_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("never-created");
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(missing.as_os_str()));

    purge_stale_scheme_once();

    assert!(!missing.exists(), "an absent cache dir must stay absent");
}

#[test]
fn evict_if_over_cap_ignores_the_scheme_sentinel() {
    // The sentinel is the marker that stops the sweep re-running; evicting it
    // would re-purge the whole cache on the next boot.
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));
    std::fs::write(tmp.path().join(scheme_sentinel_name()), vec![0u8; 500]).unwrap();
    std::fs::write(tmp.path().join("9_sm.webp"), vec![0u8; 500]).unwrap();

    evict_if_over_cap(0).unwrap();

    assert!(tmp.path().join(scheme_sentinel_name()).exists());
    assert!(!tmp.path().join("9_sm.webp").exists());
}

#[test]
fn evict_if_over_cap_removes_oldest_files() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));

    // Write 3 files with staggered mtimes (we can only guarantee ordering,
    // not specific times, so write sequentially and trust OS mtime).
    for i in 0u8..3 {
        std::fs::write(tmp.path().join(format!("{i}_sm.webp")), vec![0u8; 100]).unwrap();
        // Small sleep to ensure distinct mtimes on HFS+.
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // Cap at 200 bytes → should delete the 1 oldest file (100 bytes each, 3×100=300 total).
    evict_if_over_cap(200).unwrap();

    let remaining: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().flatten().collect();
    assert_eq!(remaining.len(), 2, "should have evicted 1 oldest file");
}

#[test]
fn touch_thumb_bumps_mtime_to_now() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));
    std::fs::write(tmp.path().join("5_sm.webp"), b"x").unwrap();
    // Rewind the mtime so a bump is observable rather than a no-op.
    let old = SystemTime::now() - std::time::Duration::from_secs(60);
    std::fs::File::open(tmp.path().join("5_sm.webp"))
        .unwrap()
        .set_modified(old)
        .unwrap();

    touch_thumb(5, ThumbSize::Sm);

    let mtime = std::fs::metadata(tmp.path().join("5_sm.webp"))
        .unwrap()
        .modified()
        .unwrap();
    assert!(mtime > old, "touch_thumb should bump mtime forward");
}

#[test]
fn evict_if_over_cap_keeps_recently_touched_file_over_an_older_untouched_one() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));

    // Explicit `set_modified` timestamps (rather than real-clock sleeps
    // between writes) keep this deterministic on filesystems with coarse
    // (e.g. whole-second) mtime resolution.
    let now = SystemTime::now();
    std::fs::write(tmp.path().join("0_sm.webp"), vec![0u8; 100]).unwrap();
    std::fs::File::open(tmp.path().join("0_sm.webp"))
        .unwrap()
        .set_modified(now - std::time::Duration::from_secs(120))
        .unwrap();
    std::fs::write(tmp.path().join("1_sm.webp"), vec![0u8; 100]).unwrap();
    std::fs::File::open(tmp.path().join("1_sm.webp"))
        .unwrap()
        .set_modified(now - std::time::Duration::from_secs(60))
        .unwrap();
    // "0" is the older-by-creation file; touching it after "1" was written
    // marks it recently-used, so eviction should take "1" instead.
    touch_thumb(0, ThumbSize::Sm);

    evict_if_over_cap(100).unwrap();

    assert!(
        tmp.path().join("0_sm.webp").exists(),
        "recently-touched file should survive eviction"
    );
    assert!(
        !tmp.path().join("1_sm.webp").exists(),
        "untouched older-relative-use file should be evicted"
    );
}

// ---------- ThumbError variants ----------

#[test]
fn generate_thumbnail_returns_failed_when_bytes_are_not_a_decodable_image() {
    // The decode step (`image::load_from_memory`) is the pipeline's first
    // fallible stage; garbage bytes that aren't any known image format must
    // surface as `ThumbError::Failed` with a message naming the decode step,
    // rather than panicking. `Failed` is the coarse variant folding the
    // former Decode/Encode/I-O cases, so the decode failure exercises it.
    let _guard = EnvVarGuard::set("OMNIBUS_THUMBS_DIR", None);
    let err = generate_thumbnail(7, ThumbSize::Sm, b"definitely not an image")
        .expect_err("undecodable bytes must not produce a thumbnail");
    assert!(
        matches!(err, ThumbError::Failed(ref msg) if msg.contains("decode")),
        "got {err:?}"
    );
}

#[test]
fn thumb_error_no_cover_renders_book_id_in_message() {
    // `NoCover` carries the book id so a caller (and the download handler)
    // can render "no cover available for book N". Constructed directly: it is
    // a coarse, caller-branchable variant whose message contract is asserted
    // here even though the current pipeline reaches the missing-cover case
    // before minting it.
    let err = ThumbError::NoCover(451);
    assert_eq!(err.to_string(), "no cover available for book 451");
}
