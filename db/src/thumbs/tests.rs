//! Unit tests for the `thumbs` module — `thumbs_dir` env-var resolution,
//! `thumb_path_for` formatting, `is_stale` mtime comparison (incl. the
//! same-second tie case), `ThumbSize` FromStr roundtrip, `thumb_etag`
//! derivation, `generate_thumbnail`'s lossy WebP output, the one-time
//! previous-scheme purge, and LRU-on-read `evict_if_over_cap` cap
//! enforcement.

use super::*;
use crate::test_support::EnvVarGuard;

/// PNG bytes for a `w`×`h` photographic stand-in: smoothly interpolated
/// value noise over a coarse lattice, which is what cover art mostly is —
/// broad tonal areas with mid-frequency detail. Neither extreme works as a
/// fixture here: a flat fill compresses to nothing under either encoder, and
/// per-pixel white noise is the one input where lossy DCT *loses*, so both
/// would make a lossy-vs-lossless comparison meaningless.
fn photographic_png(w: u32, h: u32) -> Vec<u8> {
    use image::{ImageBuffer, Rgba};

    /// Deterministic pseudo-random byte for a lattice corner.
    fn lattice(x: u32, y: u32, channel: u32) -> f32 {
        let mut n = x
            .wrapping_mul(374_761_393)
            .wrapping_add(y.wrapping_mul(668_265_263))
            .wrapping_add(channel.wrapping_mul(2_246_822_519));
        n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
        ((n >> 16) & 0xff) as f32
    }

    /// Bilinear sample of the lattice, giving smooth gradients between corners.
    fn sample(x: u32, y: u32, channel: u32) -> u8 {
        const CELL: u32 = 24;
        let (cx, cy) = (x / CELL, y / CELL);
        let (fx, fy) = (
            (x % CELL) as f32 / CELL as f32,
            (y % CELL) as f32 / CELL as f32,
        );
        let top = lattice(cx, cy, channel) * (1.0 - fx) + lattice(cx + 1, cy, channel) * fx;
        let bottom =
            lattice(cx, cy + 1, channel) * (1.0 - fx) + lattice(cx + 1, cy + 1, channel) * fx;
        (top * (1.0 - fy) + bottom * fy) as u8
    }

    let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_fn(w, h, |x, y| {
        Rgba([sample(x, y, 0), sample(x, y, 1), sample(x, y, 2), 255])
    });
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    png
}

/// The RIFF chunk fourcc at bytes 12..16 — `VP8L` for a lossless WebP,
/// `VP8 ` for a lossy one, `VP8X` for the extended container a lossy image
/// with an alpha plane is wrapped in.
fn webp_fourcc(bytes: &[u8]) -> &str {
    std::str::from_utf8(&bytes[12..16]).unwrap()
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
fn generate_thumbnail_encodes_lossy_rather_than_lossless_webp() {
    // #1750: `image`'s WebP encoder is lossless-only, so every cached thumb
    // used to be a VP8L chunk — three times the size of the cover it
    // thumbnails. The fourcc is the direct evidence of which encoder ran.
    let png = photographic_png(640, 960);
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));

    generate_thumbnail(20, ThumbSize::Lg, &png).unwrap();

    let out = std::fs::read(tmp.path().join("20_lg.webp")).unwrap();
    let fourcc = webp_fourcc(&out);
    assert_ne!(fourcc, "VP8L", "thumbnails must not be losslessly encoded");
    assert!(
        fourcc == "VP8 " || fourcc == "VP8X",
        "expected a lossy VP8 chunk, got {fourcc:?}"
    );
}

#[test]
fn generate_thumbnail_is_far_smaller_than_the_lossless_encoding() {
    // #1750 AC1/AC2: the point of the change is the byte count, so pin it
    // against the encoder it replaced rather than against an absolute number
    // that would depend on the fixture's content.
    use image::{imageops::FilterType, ImageFormat};

    let png = photographic_png(640, 960);
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));

    let lossy = generate_thumbnail(21, ThumbSize::Lg, &png).unwrap();

    let (w, h) = ThumbSize::Lg.dimensions();
    let resized = image::load_from_memory(&png)
        .unwrap()
        .resize_to_fill(w, h, FilterType::Lanczos3);
    let mut lossless = std::io::Cursor::new(Vec::new());
    resized.write_to(&mut lossless, ImageFormat::WebP).unwrap();
    let lossless = lossless.into_inner().len();

    assert!(
        lossy * 3 < lossless,
        "lossy {lossy} B should be well under a third of lossless {lossless} B"
    );
}

#[test]
fn generate_thumbnail_writes_exact_dimensions_for_every_size() {
    // #1750 AC5: the `resize_to_fill` contract — output is exactly `(w, h)`
    // — has to survive the encoder swap, or the frontend's fixed
    // `width`/`height` attributes stretch every cover.
    let png = photographic_png(400, 700);
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));

    for size in ThumbSize::all() {
        generate_thumbnail(22, size, &png).unwrap();
        let out = std::fs::read(thumb_path_for(22, size)).unwrap();
        let decoded = image::load_from_memory(&out).unwrap();
        assert_eq!(
            (decoded.width(), decoded.height()),
            size.dimensions(),
            "{size} thumbnail must be exactly its declared dimensions"
        );
    }
}

// ---------- previous-scheme purge ----------

#[test]
fn purge_stale_scheme_thumbs_once_removes_thumbs_from_a_previous_encoder() {
    // #1750 AC3: `is_stale` only compares mtime against the book's
    // `last_modified_epoch`, so a re-encode invalidates nothing on its own —
    // without this sweep an upgraded install serves its lossless thumbs
    // forever.
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));
    std::fs::write(tmp.path().join("1_sm.webp"), vec![0u8; 100]).unwrap();
    std::fs::write(tmp.path().join("1_md.webp"), vec![0u8; 100]).unwrap();
    // A sentinel from the encoder version before this one must not stop the
    // sweep, and must not survive it either.
    std::fs::write(tmp.path().join(".omnibus-thumb-scheme-v1"), b"\n").unwrap();

    assert_eq!(purge_stale_scheme_thumbs_once(), 3);

    assert!(!tmp.path().join("1_sm.webp").exists());
    assert!(!tmp.path().join("1_md.webp").exists());
    assert!(!tmp.path().join(".omnibus-thumb-scheme-v1").exists());
    assert!(tmp.path().join(scheme_sentinel_name()).exists());
}

#[cfg(unix)]
#[test]
fn purge_stale_scheme_thumbs_once_leaves_the_scheme_unmarked_when_a_removal_fails() {
    // Marking the directory current while a previous-scheme file is still in
    // it is the serve-it-forever case the sweep exists to prevent: `is_stale`
    // would go on serving that file until its book's `last_modified_epoch`
    // happened to move.
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("thumbs");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("1_sm.webp"), vec![0u8; 100]).unwrap();
    // A read-only directory fails the unlink while still allowing the walk.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();

    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(dir.as_os_str()));
    let removed = purge_stale_scheme_thumbs_once();
    let stuck = dir.join("1_sm.webp").exists();
    let marked = dir.join(scheme_sentinel_name()).exists();

    // Restore before asserting so a failure still leaves a removable tempdir.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    // Running as root ignores the mode bits, so the unlink succeeds and there
    // is no incomplete sweep to assert on.
    if !stuck {
        return;
    }
    assert_eq!(removed, 0);
    assert!(!marked, "an incomplete sweep must not mark the scheme");
}

#[test]
fn purge_stale_scheme_thumbs_once_leaves_thumbs_alone_when_the_sentinel_is_present() {
    // The sweep runs on every boot, so a second one must not keep wiping the
    // cache it just repopulated.
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));
    std::fs::write(tmp.path().join(scheme_sentinel_name()), b"\n").unwrap();
    std::fs::write(tmp.path().join("1_sm.webp"), vec![0u8; 100]).unwrap();

    assert_eq!(purge_stale_scheme_thumbs_once(), 0);

    assert!(tmp.path().join("1_sm.webp").exists());
}

#[test]
fn purge_stale_scheme_thumbs_once_marks_a_thumbs_dir_that_does_not_exist_yet() {
    // A fresh install has nothing to purge, but it still has to be marked:
    // leaving it unmarked means the *second* boot finds an unmarked directory
    // and throws away the cache the first one just built.
    let tmp = tempfile::tempdir().unwrap();
    let fresh = tmp.path().join("does-not-exist");
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(fresh.as_os_str()));

    assert_eq!(purge_stale_scheme_thumbs_once(), 0);

    assert!(fresh.join(scheme_sentinel_name()).exists());

    // …and the boot after it must leave the now-populated cache alone.
    std::fs::write(fresh.join("1_sm.webp"), vec![0u8; 100]).unwrap();
    assert_eq!(purge_stale_scheme_thumbs_once(), 0);
    assert!(fresh.join("1_sm.webp").exists());
}

#[test]
fn the_scheme_sentinel_is_neither_counted_nor_evicted() {
    // The sentinel is what makes the purge one-time. If eviction could
    // delete it, the next boot would wipe a perfectly current cache.
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));
    std::fs::write(tmp.path().join(scheme_sentinel_name()), vec![0u8; 64]).unwrap();
    std::fs::write(tmp.path().join("1_sm.webp"), vec![0u8; 100]).unwrap();

    assert_eq!(used_bytes(), 100, "sentinel bytes must not count as cache");

    evict_if_over_cap(0).unwrap();

    assert!(!tmp.path().join("1_sm.webp").exists());
    assert!(tmp.path().join(scheme_sentinel_name()).exists());
}

#[test]
fn used_bytes_sums_only_webp_files_in_the_thumbs_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(tmp.path().as_os_str()));
    std::fs::write(tmp.path().join("1_sm.webp"), vec![0u8; 100]).unwrap();
    std::fs::write(tmp.path().join("2_md.webp"), vec![0u8; 50]).unwrap();
    // A non-webp stray must not be counted.
    std::fs::write(tmp.path().join("stray.tmp"), vec![0u8; 999]).unwrap();

    assert_eq!(used_bytes(), 150);
}

#[test]
fn used_bytes_is_zero_when_the_thumbs_dir_does_not_exist() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("does-not-exist");
    let _guard = EnvVarGuard::set_os("OMNIBUS_THUMBS_DIR", Some(missing.as_os_str()));
    assert_eq!(used_bytes(), 0);
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
