//! Post-download integrity check on a freshly assembled file.
//!
//! A validator says the bytes *changed*; it cannot say the bytes you
//! assembled are *coherent*. This checks what each container can actually
//! prove, which differs sharply by format — see [`Verdict`].

use std::path::Path;

use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// Outcome of inspecting a downloaded file.
///
/// The strength of `Intact` is **not** uniform, and the difference matters
/// enough to state plainly:
///
/// - **EPUB** is a ZIP, and every member carries a CRC-32 in the central
///   directory. Reading each member to EOF verifies it, so `Intact` means
///   the bytes are the bytes the archive was built from. Any corruption is
///   caught, including a splice that preserved the archive's structure.
/// - **M4B/M4A** has no per-box checksum. The box chain is walked and must
///   tile the file exactly, which catches truncation, garbage, and any
///   splice that crosses a box header — but a same-length splice *inside*
///   `mdat` leaves every header untouched and is **not** detectable here.
/// - **MP3** is a self-syncing frame stream with no container at all, and
///   reports [`Verdict::Unverifiable`] rather than guessing.
///
/// So this is a real backstop for EPUB and a partial one for audio. What
/// keeps audio honest is the layer above: a resume always carries
/// `If-Range`, so the server refuses to splice unless *its* validator was
/// also fooled — which needs an in-place overwrite preserving length,
/// mtime and inode all at once (rule 09's documented residual).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    /// The file passed every check its format supports.
    Intact,
    /// The file is provably not what it claims to be.
    Damaged,
    /// Nothing to check: a format with no verifiable container.
    Unverifiable,
}

/// Inspect `path` according to what its extension claims it is.
pub(super) async fn verify(path: &Path) -> Verdict {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("epub") => verify_zip(path).await,
        Some("m4b" | "m4a" | "mp4") => verify_mp4(path).await,
        _ => Verdict::Unverifiable,
    }
}

/// Validate an EPUB by reading every member through the archive reader,
/// which checks each one's recorded CRC-32 as it reaches EOF.
///
/// Structure alone is not enough: two archives of equal length spliced at
/// the same offset keep a perfectly well-formed central directory whose
/// offsets all resolve, so an offsets-only check calls that intact. The
/// CRCs do not survive it.
///
/// Runs on the blocking pool — inflating a book is CPU work, and this
/// future shares the loopback runtime with the media server.
async fn verify_zip(path: &Path) -> Verdict {
    let path = path.to_path_buf();
    match tokio::task::spawn_blocking(move || verify_zip_blocking(&path)).await {
        Ok(verdict) => verdict,
        // A panic in the archive reader is not proof the file is bad.
        Err(e) => {
            tracing::warn!(error = %e, "offline download: archive check did not run");
            Verdict::Unverifiable
        }
    }
}

fn verify_zip_blocking(path: &Path) -> Verdict {
    let Ok(file) = std::fs::File::open(path) else {
        return Verdict::Damaged;
    };
    let Ok(mut archive) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
        return Verdict::Damaged;
    };
    // An EPUB always holds at least `mimetype` and `META-INF/container.xml`.
    if archive.is_empty() {
        return Verdict::Damaged;
    }
    for index in 0..archive.len() {
        let Ok(mut entry) = archive.by_index(index) else {
            return Verdict::Damaged;
        };
        // Reading to EOF is what performs the CRC comparison; the byte
        // count is irrelevant, so the output goes straight to a sink.
        if std::io::copy(&mut entry, &mut std::io::sink()).is_err() {
            return Verdict::Damaged;
        }
    }
    Verdict::Intact
}

/// Walk an M4B/M4A's flat `[u32 size][4-byte type]` box chain and require it
/// to tile the file exactly.
///
/// This catches truncation, a file that was never MP4, and a splice that
/// crosses a box header. It cannot catch a same-length splice inside
/// `mdat` — the format carries no checksum to catch it with. See
/// [`Verdict`].
async fn verify_mp4(path: &Path) -> Verdict {
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return Verdict::Damaged;
    };
    let Ok(meta) = file.metadata().await else {
        return Verdict::Damaged;
    };
    let len = meta.len();

    let mut offset = 0u64;
    let mut boxes = 0usize;
    while offset < len {
        if len - offset < 8 {
            return Verdict::Damaged;
        }
        let mut header = [0u8; 8];
        if file.seek(std::io::SeekFrom::Start(offset)).await.is_err()
            || file.read_exact(&mut header).await.is_err()
        {
            return Verdict::Damaged;
        }
        if !is_box_type(&header[4..8]) {
            return Verdict::Damaged;
        }
        let declared = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;
        let size = match declared {
            // 0 means "this box runs to the end of the file".
            0 => len - offset,
            // 1 means the real size is a 64-bit value after the type.
            1 => {
                if len - offset < 16 {
                    return Verdict::Damaged;
                }
                let mut large = [0u8; 8];
                if file.read_exact(&mut large).await.is_err() {
                    return Verdict::Damaged;
                }
                let size = u64::from_be_bytes(large);
                if size < 16 {
                    return Verdict::Damaged;
                }
                size
            }
            s if s < 8 => return Verdict::Damaged,
            s => s,
        };
        if offset.saturating_add(size) > len {
            return Verdict::Damaged;
        }
        offset += size;
        boxes += 1;
    }

    if boxes == 0 {
        Verdict::Damaged
    } else {
        Verdict::Intact
    }
}

/// Whether four bytes look like a box type.
///
/// Printable ASCII covers the standard types (`ftyp`, `moov`, `mdat`), plus
/// `0xA9` for the iTunes metadata atoms every audiobook carries — `©nam`,
/// `©ART`, `©day`. Rejecting those would call a perfectly good M4B damaged
/// and delete it.
fn is_box_type(bytes: &[u8]) -> bool {
    bytes.iter().all(|b| matches!(b, 0x20..=0x7E) || *b == 0xA9)
}

#[cfg(test)]
mod tests;
