//! Unit tests for the `thumbs` module — `thumbs_dir` env-var resolution,
//! `thumb_path_for` formatting, `is_stale` mtime comparison (incl. the
//! same-second tie case), `ThumbSize` FromStr roundtrip, `thumb_etag`
//! derivation, `generate_thumbnail` WebP output, and LRU-on-read
//! `evict_if_over_cap` cap enforcement.

use super::*;
use crate::test_support::EnvVarGuard;

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
