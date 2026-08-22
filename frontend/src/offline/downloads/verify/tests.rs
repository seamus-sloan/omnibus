//! Tests for post-download integrity checking. Splice fixtures cut at one
//! absolute offset across both files, because that is what a resume produces:
//! `resumed` bytes of the file that used to be here, then the replacement's
//! from that offset on. Equal-length pairs are the hard case — nothing about
//! the size gives them away — and each format's test says which it is.

use super::*;
use crate::offline::test_support::{zip_one, zip_one_body_offset};

/// A flat MP4/M4B box chain: `ftyp` followed by an `mdat` of `payload`.
fn mp4_with_payload(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&16u32.to_be_bytes());
    out.extend_from_slice(b"ftyp");
    out.extend_from_slice(b"M4B isom");
    out.extend_from_slice(&(8 + payload.len() as u32).to_be_bytes());
    out.extend_from_slice(b"mdat");
    out.extend_from_slice(payload);
    out
}

/// What a resume assembles when the file was replaced between attempts:
/// `resumed` bytes of `old`, then `new` from that same offset on.
fn spliced_at(old: &[u8], new: &[u8], resumed: usize) -> Vec<u8> {
    assert!(resumed < old.len() && resumed < new.len());
    let mut out = old[..resumed].to_vec();
    out.extend_from_slice(&new[resumed..]);
    assert_ne!(out, old);
    assert_ne!(
        out, new,
        "the fixture must not reassemble into a valid file"
    );
    out
}

/// The equal-length form, which is the hard case: nothing about the file's
/// size gives the splice away.
fn spliced_equal_length(old: &[u8], new: &[u8], resumed: usize) -> Vec<u8> {
    assert_eq!(old.len(), new.len(), "this fixture is about equal lengths");
    spliced_at(old, new, resumed)
}

async fn write_temp(name: &str, bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(name);
    tokio::fs::write(&path, bytes).await.expect("write");
    (dir, path)
}

// --- EPUB: CRC-backed, so any corruption is caught ---

#[tokio::test]
async fn verify_accepts_a_well_formed_epub() {
    let (_dir, path) =
        write_temp("book.epub", &zip_one(b"mimetype", b"application/epub+zip")).await;
    assert_eq!(verify(&path).await, Verdict::Intact);
}

#[tokio::test]
async fn verify_rejects_an_equal_length_epub_splice_that_keeps_the_archive_structure() {
    // The case an offsets-only check calls intact: both halves are valid
    // archives of the same length, so the surviving central directory's
    // offsets all still resolve and the end-of-central-directory record is
    // perfectly well formed. Only the member CRCs give it away.
    let old = zip_one(b"mimetype", b"application/epub+zip");
    let new = zip_one(b"mimetype", b"AAAAAAAAAAAAAAAAAAAA");
    let (_dir, path) = write_temp("book.epub", &spliced_equal_length(&old, &new, 40)).await;
    assert_eq!(verify(&path).await, Verdict::Damaged);
}

#[tokio::test]
async fn verify_rejects_a_single_flipped_byte_in_an_epub_member() {
    // The same guarantee stated at its smallest: nothing structural moves,
    // one content byte differs.
    let name: &[u8] = b"mimetype";
    let mut bytes = zip_one(name, b"application/epub+zip");
    bytes[zip_one_body_offset(name)] ^= 0xFF;
    let (_dir, path) = write_temp("book.epub", &bytes).await;
    assert_eq!(verify(&path).await, Verdict::Damaged);
}

#[tokio::test]
async fn verify_rejects_a_truncated_epub() {
    let full = zip_one(b"mimetype", b"application/epub+zip");
    let (_dir, path) = write_temp("book.epub", &full[..full.len() - 10]).await;
    assert_eq!(verify(&path).await, Verdict::Damaged);
}

#[tokio::test]
async fn verify_rejects_an_epub_that_is_not_a_zip_at_all() {
    // What a proxy's HTML error page saved under a .epub name looks like.
    let (_dir, path) = write_temp("book.epub", b"<html>504 Gateway Timeout</html>").await;
    assert_eq!(verify(&path).await, Verdict::Damaged);
}

#[tokio::test]
async fn verify_rejects_an_empty_file() {
    let (_dir, path) = write_temp("book.epub", b"").await;
    assert_eq!(verify(&path).await, Verdict::Damaged);
}

// --- M4B: structural only, and the limit is asserted rather than implied ---

#[tokio::test]
async fn verify_accepts_a_well_formed_m4b() {
    let (_dir, path) = write_temp("part-0.m4b", &mp4_with_payload(b"audio-frames")).await;
    assert_eq!(verify(&path).await, Verdict::Intact);
}

#[tokio::test]
async fn verify_accepts_the_itunes_metadata_atoms_every_audiobook_carries() {
    // `©nam` and friends start with 0xA9, which is not printable ASCII.
    // Rejecting them would call a perfectly good audiobook damaged — and
    // the caller deletes what this reports as damaged.
    let mut bytes = mp4_with_payload(b"frames");
    bytes.extend_from_slice(&14u32.to_be_bytes());
    bytes.extend_from_slice(b"\xA9nam");
    bytes.extend_from_slice(b"Title!");
    let (_dir, path) = write_temp("part-0.m4b", &bytes).await;
    assert_eq!(verify(&path).await, Verdict::Intact);
}

#[tokio::test]
async fn verify_rejects_an_m4b_whose_box_chain_overruns_the_file() {
    let mut bytes = mp4_with_payload(b"audio-frames");
    bytes.truncate(bytes.len() - 4);
    let (_dir, path) = write_temp("part-0.m4b", &bytes).await;
    assert_eq!(verify(&path).await, Verdict::Damaged);
}

#[tokio::test]
async fn verify_rejects_an_m4b_splice_that_leaves_the_mdat_size_wrong() {
    // The detectable audio case: the replacement is a different length, so
    // the `mdat` size the walk reads (from the old half) no longer matches
    // the bytes that follow it and the chain stops tiling the file.
    //
    // Only unequal lengths produce this. Between two same-length files
    // every box header is byte-identical, so no cut point yields anything
    // but the replacement itself — which is the limitation the test below
    // pins.
    let old = mp4_with_payload(b"short-original");
    let new = mp4_with_payload(b"a-considerably-longer-replacement-recording");
    let (_dir, path) = write_temp("part-0.m4b", &spliced_at(&old, &new, 20)).await;
    assert_eq!(verify(&path).await, Verdict::Damaged);
}

#[tokio::test]
async fn verify_cannot_detect_an_equal_length_m4b_splice_inside_mdat() {
    // Documented limitation, pinned so it is a known cost rather than a
    // surprise: MP4 carries no per-box checksum, so a splice that leaves
    // every header intact is invisible to any structural walk. What keeps
    // this from mattering is the layer above — a resume carries `If-Range`,
    // so reaching this state at all needs the *server's* validator to have
    // been fooled too.
    let old = mp4_with_payload(b"the-original-recording-payload");
    let new = mp4_with_payload(b"a-different-recording-payload!");
    let spliced = spliced_equal_length(&old, &new, 30);
    let (_dir, path) = write_temp("part-0.m4b", &spliced).await;
    assert_eq!(
        verify(&path).await,
        Verdict::Intact,
        "if this ever fails, MP4 detection improved — update Verdict's doc"
    );
}

#[tokio::test]
async fn verify_reports_unverifiable_for_a_format_with_no_container() {
    let (_dir, path) = write_temp("part-0.mp3", b"\xff\xfb\x90\x00 frames").await;
    assert_eq!(verify(&path).await, Verdict::Unverifiable);
}

#[tokio::test]
async fn verify_reports_damaged_for_a_missing_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_eq!(
        verify(&dir.path().join("absent.epub")).await,
        Verdict::Damaged
    );
}
