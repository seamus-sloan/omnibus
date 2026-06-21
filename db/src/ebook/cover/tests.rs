use crate::ebook::test_support::*;
use crate::ebook::{scan_ebook_library, scan_ebook_library_with, ScanOptions};

#[test]
fn extract_metadata_uses_sidecar_when_present() {
    // alpha.epub ships an embedded cover. Plant a recognizably-different
    // sidecar next to it; the scanner must return the sidecar bytes.
    let dir = make_test_dir("sidecar_wins");
    copy_fixture_into("alpha.epub", &dir);
    let sidecar_bytes: &[u8] = b"sidecar-jpg-magic-bytes";
    std::fs::write(dir.join("alpha.jpg"), sidecar_bytes).unwrap();

    let out = scan_ebook_library(Some(dir.to_str().unwrap()));
    std::fs::remove_dir_all(&dir).unwrap();

    let alpha = out
        .books
        .iter()
        .find(|b| b.metadata.filename == "alpha.epub")
        .expect("alpha present");
    let (mime, bytes) = alpha.cover.as_ref().expect("cover present");
    assert_eq!(bytes, sidecar_bytes, "expected sidecar bytes, got embedded");
    assert_eq!(mime, "image/jpeg");
}

#[test]
fn extract_metadata_uses_embedded_when_no_sidecar() {
    // alpha.epub has an embedded cover; no sidecar planted. We don't
    // know the exact embedded bytes, but they should be non-empty and
    // the cover slot must be populated. Default ScanOptions disables
    // materialization, so no sidecar should appear after the scan.
    let dir = make_test_dir("embedded_only");
    copy_fixture_into("alpha.epub", &dir);

    let out = scan_ebook_library(Some(dir.to_str().unwrap()));
    let sidecar_appeared = find_materialized_sidecar(&dir, "alpha").is_some();
    std::fs::remove_dir_all(&dir).unwrap();

    assert!(
        !sidecar_appeared,
        "default ScanOptions must not materialize sidecars"
    );
    let alpha = out
        .books
        .iter()
        .find(|b| b.metadata.filename == "alpha.epub")
        .expect("alpha present");
    let (_, bytes) = alpha.cover.as_ref().expect("embedded cover present");
    assert!(!bytes.is_empty());
}

#[test]
fn extract_metadata_materializes_sidecar_with_opt_in() {
    // With `materialize_sidecars: true`, scanning an epub that has an
    // embedded cover but no sidecar must write `<basename>.{jpg|png}`
    // (extension matches embedded mime) next to the file so subsequent
    // scans hit the sidecar directly.
    let dir = make_test_dir("materialize");
    copy_fixture_into("alpha.epub", &dir);
    assert!(
        find_materialized_sidecar(&dir, "alpha").is_none(),
        "precondition: no sidecar yet"
    );

    let out = scan_ebook_library_with(
        Some(dir.to_str().unwrap()),
        ScanOptions {
            materialize_sidecars: true,
        },
    );

    let sidecar = find_materialized_sidecar(&dir, "alpha");
    let written = sidecar.as_ref().and_then(|p| std::fs::read(p).ok());
    let alpha = out
        .books
        .iter()
        .find(|b| b.metadata.filename == "alpha.epub")
        .map(|b| b.cover.as_ref().map(|(_, bytes)| bytes.clone()))
        .unwrap_or(None);
    std::fs::remove_dir_all(&dir).unwrap();

    assert!(sidecar.is_some(), "sidecar should have been written");
    assert_eq!(
        written.as_deref(),
        alpha.as_deref(),
        "written sidecar bytes must match returned cover bytes"
    );
}

#[test]
fn extract_metadata_second_scan_reads_sidecar_not_zip() {
    // After materialization, swap the sidecar with different bytes. The
    // next scan should return *those* bytes, proving the read came from
    // the sidecar and not the unchanged embedded cover in the zip.
    let dir = make_test_dir("second_scan");
    copy_fixture_into("alpha.epub", &dir);

    // First scan: materialize.
    let _ = scan_ebook_library_with(
        Some(dir.to_str().unwrap()),
        ScanOptions {
            materialize_sidecars: true,
        },
    );

    let sidecar_path =
        find_materialized_sidecar(&dir, "alpha").expect("first scan materialized a sidecar");

    // Replace the sidecar (same path/extension) with sentinel bytes.
    let sentinel: &[u8] = b"replaced-after-materialization";
    std::fs::write(&sidecar_path, sentinel).unwrap();

    // Second scan (default opts) — should read the sentinel, not
    // re-extract the embedded cover.
    let out = scan_ebook_library(Some(dir.to_str().unwrap()));
    std::fs::remove_dir_all(&dir).unwrap();

    let alpha = out
        .books
        .iter()
        .find(|b| b.metadata.filename == "alpha.epub")
        .expect("alpha present");
    let (_, bytes) = alpha.cover.as_ref().expect("cover present");
    assert_eq!(
        bytes, sentinel,
        "second scan should have read the swapped sidecar"
    );
}

#[test]
fn extract_metadata_repairs_unreadable_sidecar_on_materialize() {
    // alpha.epub has an embedded cover. Plant a zero-length sidecar that
    // sidecar_cover_for() will pick up but read_sidecar() can't use.
    // With materialize_sidecars=true, the broken cache must be repaired
    // so the next scan reads the sidecar instead of re-opening the zip.
    let dir = make_test_dir("repair_sidecar");
    copy_fixture_into("alpha.epub", &dir);

    // alpha.epub embeds a PNG, so the materializer would write
    // `alpha.png`. Plant the corrupt sidecar at that exact path.
    let broken = dir.join("alpha.png");
    std::fs::write(&broken, b"").unwrap();

    let out = scan_ebook_library_with(
        Some(dir.to_str().unwrap()),
        ScanOptions {
            materialize_sidecars: true,
        },
    );

    let repaired_bytes = std::fs::read(&broken).expect("sidecar still on disk");
    let alpha_cover = out
        .books
        .iter()
        .find(|b| b.metadata.filename == "alpha.epub")
        .and_then(|b| b.cover.as_ref().map(|(_, bytes)| bytes.clone()));
    std::fs::remove_dir_all(&dir).unwrap();

    assert!(
        !repaired_bytes.is_empty(),
        "broken zero-length sidecar should have been repaired"
    );
    assert_eq!(
        alpha_cover.as_deref(),
        Some(repaired_bytes.as_slice()),
        "repaired sidecar bytes must match the embedded cover the scan returned"
    );
}

#[test]
fn extract_metadata_does_not_clobber_unrelated_existing_sidecar() {
    // The repair gate must only overwrite the *exact* corrupt file the
    // sidecar lookup returned — never a different valid file that
    // happens to sit at the materialize target.
    //
    // Setup: alpha.epub embeds a PNG, so materialize would target
    // alpha.png. We plant a corrupt (empty) `alpha.jpg` (which jpg-over-
    // png priority makes sidecar_cover_for return) AND a valid `alpha.png`
    // (the user's curated cover). The materialize step must refuse to
    // overwrite alpha.png because the *known* corrupt path is alpha.jpg.
    let dir = make_test_dir("no_clobber");
    copy_fixture_into("alpha.epub", &dir);

    let corrupt_jpg = dir.join("alpha.jpg");
    std::fs::write(&corrupt_jpg, b"").unwrap();
    let valid_png = dir.join("alpha.png");
    let curated: &[u8] = b"user-curated-cover-do-not-touch";
    std::fs::write(&valid_png, curated).unwrap();

    let _ = scan_ebook_library_with(
        Some(dir.to_str().unwrap()),
        ScanOptions {
            materialize_sidecars: true,
        },
    );

    let png_after = std::fs::read(&valid_png).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();

    assert_eq!(
        png_after, curated,
        "alpha.png is not the corrupt sidecar — must not be overwritten"
    );
}

#[test]
fn extract_metadata_no_embedded_no_sidecar_returns_none() {
    // gamma.epub has no embedded cover. No sidecar planted, no
    // materialization. Cover should stay None and no file should be
    // written.
    let dir = make_test_dir("no_cover");
    copy_fixture_into("gamma.epub", &dir);

    let out = scan_ebook_library_with(
        Some(dir.to_str().unwrap()),
        ScanOptions {
            materialize_sidecars: true,
        },
    );
    let sidecar_appeared = find_materialized_sidecar(&dir, "gamma").is_some();
    std::fs::remove_dir_all(&dir).unwrap();

    assert!(
        !sidecar_appeared,
        "no embedded cover → nothing to materialize"
    );
    let gamma = out
        .books
        .iter()
        .find(|b| b.metadata.filename == "gamma.epub")
        .expect("gamma present");
    assert!(gamma.cover.is_none());
}

#[cfg(unix)]
#[test]
fn extract_metadata_materialization_failure_falls_back_to_embedded() {
    // chmod the directory read-only-execute so write fails. The scanner
    // must still return cover bytes (from embedded), and no sidecar
    // should appear.
    use std::os::unix::fs::PermissionsExt;

    let dir = make_test_dir("readonly_dir");
    copy_fixture_into("alpha.epub", &dir);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();

    // Skip if the chmod didn't take (e.g. running as root in some CI
    // containers).
    if std::fs::write(dir.join("write_probe"), b"x").is_ok() {
        std::fs::remove_file(dir.join("write_probe")).ok();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        return;
    }

    let out = scan_ebook_library_with(
        Some(dir.to_str().unwrap()),
        ScanOptions {
            materialize_sidecars: true,
        },
    );

    let sidecar_appeared = find_materialized_sidecar(&dir, "alpha").is_some();

    // Restore perms before cleanup.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();

    assert!(!sidecar_appeared, "read-only fs must not produce a sidecar");
    let alpha = out
        .books
        .iter()
        .find(|b| b.metadata.filename == "alpha.epub")
        .expect("alpha present");
    let (_, bytes) = alpha.cover.as_ref().expect("embedded fallback present");
    assert!(!bytes.is_empty(), "embedded fallback must be non-empty");
}
