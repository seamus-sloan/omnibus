//! Unit tests for the `journal_images` module — write/read round-trip,
//! unsupported-mime rejection, path-traversal/malformed-name rejection, and
//! the orphan-cleanup helpers (`referenced_image_names`, `delete_journal_image`).

use super::*;
use crate::test_support::EnvVarGuard;

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
