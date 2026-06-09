//! Unit tests for the `thumbs` module — `thumbs_dir` env-var resolution,
//! `thumb_path_for` formatting, `is_stale` mtime comparison,
//! `ThumbSize` FromStr roundtrip, `generate_thumbnail` WebP output, and
//! FIFO-by-mtime `evict_if_over_cap` cap enforcement.

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
    let _guard = EnvVarGuard::set("OMNIBUS_THUMBS_DIR", Some(tmp.path().to_str().unwrap()));
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
fn is_stale_returns_true_when_mtime_is_older() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set("OMNIBUS_THUMBS_DIR", Some(tmp.path().to_str().unwrap()));
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
    let _guard = EnvVarGuard::set("OMNIBUS_THUMBS_DIR", Some(tmp.path().to_str().unwrap()));
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
    let _guard = EnvVarGuard::set("OMNIBUS_THUMBS_DIR", Some(tmp.path().to_str().unwrap()));

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
