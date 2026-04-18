use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LibrarySection {
    pub path: Option<String>,
    pub files: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryContents {
    pub ebooks: LibrarySection,
    pub audiobooks: LibrarySection,
}

pub fn list_files(path: Option<&str>) -> LibrarySection {
    let Some(path_str) = path else {
        return LibrarySection::default();
    };

    let path = std::path::Path::new(path_str);
    if !path.exists() {
        return LibrarySection {
            path: Some(path_str.to_string()),
            files: vec![],
            error: Some(format!("path not found: {path_str}")),
        };
    }

    match std::fs::read_dir(path) {
        Err(e) => LibrarySection {
            path: Some(path_str.to_string()),
            files: vec![],
            error: Some(format!("could not read directory: {e}")),
        },
        Ok(entries) => {
            let mut files: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                .filter_map(|e| e.file_name().into_string().ok())
                .collect();
            files.sort();
            LibrarySection {
                path: Some(path_str.to_string()),
                files,
                error: None,
            }
        }
    }
}

pub fn scan_libraries(ebook_path: Option<&str>, audiobook_path: Option<&str>) -> LibraryContents {
    LibraryContents {
        ebooks: list_files(ebook_path),
        audiobooks: list_files(audiobook_path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_test_dir(suffix: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("omnibus_scanner_test_{suffix}"));
        fs::create_dir_all(&dir).expect("should create test dir");
        dir
    }

    #[test]
    fn list_files_with_no_path_returns_empty() {
        let result = list_files(None);
        assert_eq!(result.path, None);
        assert!(result.files.is_empty());
        assert!(result.error.is_none());
    }

    #[test]
    fn list_files_with_nonexistent_path_returns_error() {
        let result = list_files(Some("/definitely/does/not/exist/omnibus_test"));
        assert!(result.error.is_some());
        assert!(result.files.is_empty());
    }

    #[test]
    fn list_files_returns_sorted_filenames() {
        let dir = make_test_dir("sorted");
        fs::write(dir.join("c.epub"), b"").unwrap();
        fs::write(dir.join("a.epub"), b"").unwrap();
        fs::write(dir.join("b.m4b"), b"").unwrap();
        let result = list_files(Some(dir.to_str().unwrap()));
        fs::remove_dir_all(&dir).unwrap();
        assert!(result.error.is_none());
        assert_eq!(result.files, vec!["a.epub", "b.m4b", "c.epub"]);
    }

    #[test]
    fn list_files_does_not_include_subdirectories() {
        let dir = make_test_dir("subdirs");
        fs::write(dir.join("book.epub"), b"").unwrap();
        fs::create_dir(dir.join("subdir")).unwrap();
        let result = list_files(Some(dir.to_str().unwrap()));
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(result.files, vec!["book.epub"]);
    }

    #[test]
    fn scan_libraries_combines_both_sections() {
        let dir = make_test_dir("combined");
        fs::write(dir.join("novel.epub"), b"").unwrap();
        let path = dir.to_str().unwrap();
        let result = scan_libraries(Some(path), None);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(result.ebooks.files, vec!["novel.epub"]);
        assert!(result.audiobooks.path.is_none());
    }
}
