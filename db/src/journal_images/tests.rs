use super::*;

/// Serialize the env-var mutation across tests in this module.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn with_temp_dir<T>(f: impl FnOnce() -> T) -> T {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!("omnibus-ji-test-{}", uuid::Uuid::new_v4()));
    std::env::set_var("OMNIBUS_JOURNAL_IMAGES_DIR", &dir);
    let out = f();
    std::env::remove_var("OMNIBUS_JOURNAL_IMAGES_DIR");
    let _ = std::fs::remove_dir_all(dir);
    out
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
