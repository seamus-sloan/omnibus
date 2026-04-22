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
                    .map(|e| e.to_lowercase())
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

pub const EBOOK_EXTENSIONS: &[&str] = &["epub", "pdf"];
pub const AUDIOBOOK_EXTENSIONS: &[&str] = &["m4b", "mp3"];

pub fn scan_libraries(ebook_path: Option<&str>, audiobook_path: Option<&str>) -> LibraryContents {
    LibraryContents {
        ebooks: list_files(ebook_path, EBOOK_EXTENSIONS),
        audiobooks: list_files(audiobook_path, AUDIOBOOK_EXTENSIONS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_test_dir(suffix: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("omnibus_scanner_test_{suffix}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("should create test dir");
        dir
    }

    #[test]
    fn list_files_with_no_path_returns_empty() {
        let result = list_files(None, EBOOK_EXTENSIONS);
        assert_eq!(result.path, None);
        assert_eq!(result.total_files, 0);
        assert!(result.counts_by_ext.is_empty());
        assert!(result.error.is_none());
    }

    #[test]
    fn list_files_with_nonexistent_path_returns_error() {
        let result = list_files(
            Some("/definitely/does/not/exist/omnibus_test"),
            EBOOK_EXTENSIONS,
        );
        assert!(result.error.is_some());
        assert_eq!(result.total_files, 0);
    }

    #[test]
    fn list_files_counts_by_extension() {
        let dir = make_test_dir("counts");
        fs::write(dir.join("a.epub"), b"").unwrap();
        fs::write(dir.join("b.epub"), b"").unwrap();
        fs::write(dir.join("c.pdf"), b"").unwrap();
        fs::write(dir.join("d.txt"), b"").unwrap();
        let result = list_files(Some(dir.to_str().unwrap()), EBOOK_EXTENSIONS);
        fs::remove_dir_all(&dir).unwrap();
        assert!(result.error.is_none());
        assert_eq!(result.total_files, 4);
        assert_eq!(
            result.counts_by_ext,
            vec![("epub".to_string(), 2), ("pdf".to_string(), 1)]
        );
    }

    #[test]
    fn list_files_recurses_into_subdirectories() {
        let dir = make_test_dir("recursive");
        fs::write(dir.join("top.epub"), b"").unwrap();
        let nested = dir.join("series").join("vol1");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("deep.epub"), b"").unwrap();
        fs::write(nested.join("cover.jpg"), b"").unwrap();
        let result = list_files(Some(dir.to_str().unwrap()), EBOOK_EXTENSIONS);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(result.total_files, 3);
        assert_eq!(
            result.counts_by_ext,
            vec![("epub".to_string(), 2), ("pdf".to_string(), 0)]
        );
    }

    #[test]
    fn list_files_extension_match_is_case_insensitive() {
        let dir = make_test_dir("case");
        fs::write(dir.join("A.EPUB"), b"").unwrap();
        fs::write(dir.join("b.Pdf"), b"").unwrap();
        let result = list_files(Some(dir.to_str().unwrap()), EBOOK_EXTENSIONS);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            result.counts_by_ext,
            vec![("epub".to_string(), 1), ("pdf".to_string(), 1)]
        );
    }

    #[test]
    fn scan_libraries_uses_audiobook_extensions() {
        let dir = make_test_dir("audiobooks");
        fs::write(dir.join("chapter1.m4b"), b"").unwrap();
        fs::write(dir.join("chapter2.mp3"), b"").unwrap();
        fs::write(dir.join("chapter3.mp3"), b"").unwrap();
        let path = dir.to_str().unwrap();
        let result = scan_libraries(None, Some(path));
        fs::remove_dir_all(&dir).unwrap();
        assert!(result.ebooks.path.is_none());
        assert_eq!(result.audiobooks.total_files, 3);
        assert_eq!(
            result.audiobooks.counts_by_ext,
            vec![("m4b".to_string(), 1), ("mp3".to_string(), 2)]
        );
    }
}
