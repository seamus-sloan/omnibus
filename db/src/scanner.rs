//! Recursive library-directory scanner.
//!
//! Walks a configured library root and returns per-extension file counts.
//! Used by the settings page to surface live summaries and by
//! [`crate::indexer`] when deciding what to reindex.

pub use omnibus_shared::{LibraryContents, LibrarySection};

/// Recursively walk `path` and return total file count plus per-extension
/// counts for each extension in `extensions` (compared case-insensitively,
/// without leading dot — e.g. `&["epub", "pdf"]`).
pub fn list_files(path: Option<&str>, extensions: &[&str]) -> LibrarySection {
    let Some(path_str) = path else {
        return LibrarySection::default();
    };

    let root = std::path::Path::new(path_str);
    if !root.exists() {
        return LibrarySection {
            path: Some(path_str.to_string()),
            total_files: 0,
            counts_by_ext: extensions.iter().map(|e| (e.to_string(), 0)).collect(),
            error: Some(format!("path not found: {path_str}")),
        };
    }

    let mut total_files: usize = 0;
    let mut counts: Vec<(String, usize)> = extensions
        .iter()
        .map(|e| (e.to_lowercase(), 0usize))
        .collect();

    let mut stack: Vec<std::path::PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                return LibrarySection {
                    path: Some(path_str.to_string()),
                    total_files: 0,
                    counts_by_ext: extensions.iter().map(|e| (e.to_string(), 0)).collect(),
                    error: Some(format!("could not read directory: {e}")),
                };
            }
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                total_files += 1;
                if let Some(ext) = entry
                    .path()
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_lowercase)
                {
                    if let Some(slot) = counts.iter_mut().find(|(key, _)| key == &ext) {
                        slot.1 += 1;
                    }
                }
            }
        }
    }

    LibrarySection {
        path: Some(path_str.to_string()),
        total_files,
        counts_by_ext: counts,
        error: None,
    }
}

/// File extensions the ebook library walk accepts.
pub const EBOOK_EXTENSIONS: &[&str] = &["epub", "pdf", "cbz"];
/// File extensions the audiobook library walk accepts.
pub const AUDIOBOOK_EXTENSIONS: &[&str] = &["m4b", "mp3"];

/// Scan all configured library directories and return a combined `LibraryContents` with ebook and audiobook stat entries.
pub fn scan_libraries(ebook_path: Option<&str>, audiobook_path: Option<&str>) -> LibraryContents {
    LibraryContents {
        ebooks: list_files(ebook_path, EBOOK_EXTENSIONS),
        audiobooks: list_files(audiobook_path, AUDIOBOOK_EXTENSIONS),
    }
}

#[cfg(test)]
mod tests;
