//! Copy a source EPUB zip to a new archive, swapping only the OPF and cover
//! entries (F5.8 #1372). Every other entry — text, styles, fonts, nav — is
//! copied through unchanged, and the `mimetype` entry is re-emitted first and
//! uncompressed per the EPUB OCF spec so the result stays a valid container.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::Context;
use zip::read::ZipFile;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// Hard cap on the bytes read from any single zip entry. A crafted EPUB can
/// declare an arbitrary (and false) [`ZipFile::size`], or rely on deflate's
/// ~1000:1 worst-case ratio to make a small archive decompress to gigabytes
/// (a "zip bomb") — this bounds memory regardless of what the entry claims
/// or how well it compresses. 200 MiB comfortably covers any real EPUB
/// resource (fonts, images, video) while staying far below an OOM.
const MAX_ENTRY_BYTES: u64 = 200 * 1024 * 1024;

/// Read `entry` fully into memory, capped at [`MAX_ENTRY_BYTES`] regardless
/// of the entry's declared (attacker-controlled) size. Errors if the entry's
/// actual decompressed content exceeds the cap.
fn read_entry_bounded<R: Read>(entry: &mut ZipFile<'_, R>, name: &str) -> anyhow::Result<Vec<u8>> {
    let hint = entry.size().min(MAX_ENTRY_BYTES) as usize;
    let mut raw = Vec::with_capacity(hint);
    // Take one byte past the cap so an entry that lands exactly on the
    // boundary reads clean, while anything larger is distinguishable from a
    // legitimately-sized entry.
    let read = entry
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut raw)
        .with_context(|| format!("read entry {name}"))?;
    if read as u64 > MAX_ENTRY_BYTES {
        anyhow::bail!("zip entry {name} exceeds {MAX_ENTRY_BYTES} byte cap");
    }
    Ok(raw)
}

/// Rewrite `src` into `dst`, replacing the `opf_path` entry with the output of
/// `opf_transform` and — when `cover` is `Some((path, bytes))` — the cover
/// entry at `path` with `bytes`. All other entries are copied verbatim.
///
/// `opf_transform` runs on the raw OPF bytes read from the archive; its failure
/// aborts the whole rewrite (a half-written `dst` is the caller's to discard —
/// it writes to a temp path and only promotes on success).
pub(super) fn rewrite_archive(
    src: &Path,
    dst: &Path,
    opf_path: &str,
    opf_transform: impl Fn(&[u8]) -> anyhow::Result<Vec<u8>>,
    cover: Option<(String, Vec<u8>)>,
) -> anyhow::Result<()> {
    let mut archive = ZipArchive::new(File::open(src).context("open source epub")?)
        .context("read source epub as zip")?;
    let out = File::create(dst).context("create rewritten epub")?;
    let mut writer = ZipWriter::new(out);

    // EPUB OCF requires `mimetype` to be the archive's *first* entry, stored
    // uncompressed. Emit it explicitly up front so the output is spec-correct
    // even when the source stored it out of order (we can't rely on the source
    // iteration order); the main loop then skips it.
    if let Ok(mut mt) = archive.by_name("mimetype") {
        let raw = read_entry_bounded(&mut mt, "mimetype")?;
        drop(mt);
        writer
            .start_file(
                "mimetype",
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
            )
            .context("start mimetype entry")?;
        writer.write_all(&raw).context("write mimetype entry")?;
    }

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).context("read zip entry")?;
        let name = entry.name().to_owned();

        // Already emitted first, above.
        if name == "mimetype" {
            continue;
        }

        if entry.is_dir() {
            writer
                .add_directory(&name, SimpleFileOptions::default())
                .with_context(|| format!("write dir entry {name}"))?;
            continue;
        }

        // Every non-mimetype entry deflates.
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        let bytes = if name == opf_path {
            let raw = read_entry_bounded(&mut entry, "OPF entry")?;
            opf_transform(&raw)?
        } else if cover.as_ref().is_some_and(|(p, _)| *p == name) {
            // Safe: matched the guard above.
            cover.as_ref().map(|(_, b)| b.clone()).unwrap_or_default()
        } else {
            read_entry_bounded(&mut entry, &name)?
        };

        writer
            .start_file(&name, options)
            .with_context(|| format!("start entry {name}"))?;
        writer
            .write_all(&bytes)
            .with_context(|| format!("write entry {name}"))?;
    }

    writer.finish().context("finalize rewritten epub")?;
    Ok(())
}
