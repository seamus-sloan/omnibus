//! Unit tests for the `journal_images` module — write/read round-trip,
//! unsupported-mime rejection, path-traversal/malformed-name rejection, the
//! orphan-cleanup helpers (`referenced_image_names`, `delete_journal_image`),
//! and the boot-time relocation out of the `$OMNIBUS_DATA_DIR` default.

use super::*;
use crate::test_support::EnvVarGuard;

/// A name of the exact `<uuidv4>.<ext>` shape `write_journal_image` mints.
const IMAGE_NAME: &str = "0b0c8bcc-2f5c-4f8e-9df1-0a2f4e21a111.png";

/// Point `OMNIBUS_JOURNAL_IMAGES_DIR` at a fresh temp dir for the duration
/// of `f`. Both the env var (via `EnvVarGuard`) and the temp dir (via
/// `tempfile::TempDir`) restore/clean up on drop, including on panic.
fn with_temp_dir<T>(f: impl FnOnce() -> T) -> T {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_JOURNAL_IMAGES_DIR", Some(tmp.path().as_os_str()));
    f()
}

#[test]
fn write_then_read_round_trips_bytes_and_mime() {
    with_temp_dir(|| {
        let name = write_journal_image("image/png", b"png-bytes").unwrap();
        assert!(name.ends_with(".png"), "got: {name}");
        let (mime, bytes) = read_journal_image(&name).expect("readable");
        assert_eq!(mime, "image/png");
        assert_eq!(bytes, b"png-bytes");
    });
}

#[test]
fn write_journal_image_rejects_unsupported_mime() {
    with_temp_dir(|| {
        let err = write_journal_image("image/svg+xml", b"<svg/>").unwrap_err();
        assert!(err.to_string().contains("unsupported"), "got: {err}");
    });
}

#[test]
fn read_journal_image_rejects_traversal_and_malformed_names() {
    with_temp_dir(|| {
        for name in [
            "../secrets.png",
            "..%2Fsecrets.png",
            "nope.png",
            "a/b.png",
            "0b0c8bcc-2f5c-4f8e-9df1-0a2f4e21a111.svg",
            "0b0c8bcc-2f5c-4f8e-9df1-0a2f4e21a111.png.exe",
        ] {
            assert!(read_journal_image(name).is_none(), "must reject {name}");
        }
    });
}

#[test]
fn referenced_image_names_extracts_every_embed_url_and_ignores_the_rest() {
    let a = format!(
        "{}0b0c8bcc-2f5c-4f8e-9df1-0a2f4e21a111.png",
        crate::journals::markdown::IMAGE_URL_PREFIX
    );
    let b = format!(
        "{}1c1d9ddd-3f6d-5f9f-0e02-1b3f5f32b222.jpg",
        crate::journals::markdown::IMAGE_URL_PREFIX
    );
    let body =
        format!("![alt]({a}) some text ![alt2]({b}) and a bogus one https://evil.example/x.png");
    let names = referenced_image_names(&body);
    assert_eq!(names.len(), 2, "got: {names:?}");
    assert!(names.contains("0b0c8bcc-2f5c-4f8e-9df1-0a2f4e21a111.png"));
    assert!(names.contains("1c1d9ddd-3f6d-5f9f-0e02-1b3f5f32b222.jpg"));
}

#[test]
fn referenced_image_names_ignores_malformed_names_and_empty_body() {
    let body = format!(
        "{}not-a-uuid.png",
        crate::journals::markdown::IMAGE_URL_PREFIX
    );
    assert!(referenced_image_names(&body).is_empty());
    assert!(referenced_image_names("").is_empty());
}

#[test]
fn delete_journal_image_removes_an_existing_file_and_is_a_noop_for_a_missing_one() {
    with_temp_dir(|| {
        let name = write_journal_image("image/png", b"bytes").unwrap();
        assert!(read_journal_image(&name).is_some());

        delete_journal_image(&name);
        assert!(read_journal_image(&name).is_none());

        // Deleting again (already gone) must not panic.
        delete_journal_image(&name);
    });
}

/// Seed a `$OMNIBUS_DATA_DIR/journal-images` holding one file, and pin both
/// vars for the call — `dest` of `None` is the no-override case, where the
/// configured directory *is* the one being relocated from.
fn with_legacy_image<T>(
    name: &str,
    bytes: &[u8],
    dest: Option<&Path>,
    f: impl FnOnce(&Path) -> T,
) -> T {
    let data = tempfile::tempdir().unwrap();
    let legacy = data.path().join("journal-images");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join(name), bytes).unwrap();

    let _guard = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(data.path().as_os_str()))
        .also_set_os("OMNIBUS_JOURNAL_IMAGES_DIR", dest.map(|d| d.as_os_str()));
    f(&legacy)
}

#[test]
fn relocate_legacy_journal_images_moves_images_out_of_the_data_dir_default() {
    let dest = tempfile::tempdir().unwrap();
    with_legacy_image(IMAGE_NAME, b"png-bytes", Some(dest.path()), |legacy| {
        relocate_legacy_journal_images();

        let (_, bytes) = read_journal_image(IMAGE_NAME).expect("readable at the new location");
        assert_eq!(bytes, b"png-bytes");
        assert!(!legacy.exists(), "an emptied legacy dir must be removed");
    });
}

#[test]
fn relocate_legacy_journal_images_is_a_noop_when_no_override_is_set() {
    with_legacy_image(IMAGE_NAME, b"png-bytes", None, |legacy| {
        relocate_legacy_journal_images();

        assert!(
            legacy.join(IMAGE_NAME).exists(),
            "the only copy must stay where it is"
        );
        assert!(read_journal_image(IMAGE_NAME).is_some());
    });
}

#[test]
fn relocate_legacy_journal_images_keeps_the_destination_copy_on_a_name_collision() {
    let dest = tempfile::tempdir().unwrap();
    std::fs::write(dest.path().join(IMAGE_NAME), b"current").unwrap();

    with_legacy_image(IMAGE_NAME, b"stale", Some(dest.path()), |legacy| {
        relocate_legacy_journal_images();

        let (_, bytes) = read_journal_image(IMAGE_NAME).expect("readable");
        assert_eq!(bytes, b"current", "the destination copy must win");
        // Never unlink a file we didn't move — the /cache wipe takes it.
        assert!(legacy.join(IMAGE_NAME).exists());
    });
}

#[test]
fn relocate_legacy_journal_images_leaves_names_it_did_not_mint_behind() {
    let dest = tempfile::tempdir().unwrap();
    with_legacy_image("notes.txt", b"not ours", Some(dest.path()), |legacy| {
        relocate_legacy_journal_images();

        assert!(legacy.join("notes.txt").exists(), "must stay put");
        assert!(!dest.path().join("notes.txt").exists(), "must not be swept");
    });
}

#[test]
fn relocate_legacy_journal_images_is_a_noop_when_the_legacy_dir_is_absent() {
    let data = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let dest_marker = dest.path().join(IMAGE_NAME);
    std::fs::write(&dest_marker, b"png-bytes").unwrap();

    let _guard = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(data.path().as_os_str()))
        .also_set_os("OMNIBUS_JOURNAL_IMAGES_DIR", Some(dest.path().as_os_str()));

    relocate_legacy_journal_images();

    assert!(dest_marker.exists(), "the configured dir must be untouched");
}

/// The `rename` in `move_across_volumes` succeeds whenever both sides share a
/// filesystem, which every test temp dir does — so the fallback, which is the
/// only path a Docker upgrade takes across the `/cache` and `/config` mounts,
/// is exercised directly here rather than through the boot hook.
#[test]
fn copy_then_unlink_moves_the_bytes_and_leaves_no_temp_file() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let from = src.path().join(IMAGE_NAME);
    let to = dst.path().join(IMAGE_NAME);
    std::fs::write(&from, b"png-bytes").unwrap();

    copy_then_unlink(&from, &to).expect("cross-volume move");

    assert_eq!(std::fs::read(&to).unwrap(), b"png-bytes");
    assert!(!from.exists(), "the source is unlinked once the copy lands");
    let left: Vec<_> = std::fs::read_dir(dst.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(
        left,
        [std::ffi::OsString::from(IMAGE_NAME)],
        "no .part may survive"
    );
}

#[test]
fn copy_then_unlink_leaves_no_temp_file_when_the_copy_fails() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let from = src.path().join(IMAGE_NAME); // deliberately never created
    let to = dst.path().join(IMAGE_NAME);

    copy_then_unlink(&from, &to).expect_err("a missing source must error");

    assert_eq!(
        std::fs::read_dir(dst.path()).unwrap().count(),
        0,
        "a failed copy must not leave a .part behind"
    );
}
