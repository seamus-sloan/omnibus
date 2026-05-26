//! Inline integration tests for the db query layer. Lives here as a
//! single file (rather than co-located per module) because so many tests
//! exercise behavior that spans two or three of the new modules
//! (covers + sync, settings + cover cleanup, overrides + book reads).
//! Splitting them per module would force every test helper out into
//! `pub(crate)` and obscure what's actually under test. Future tests for
//! a single module's internal behavior should still land in that
//! module's inline `#[cfg(test)] mod tests`.

#[allow(unused_imports, dead_code)]
mod tests {
    use crate::author_photos_data::{
        author_photo_status, delete_author, delete_author_photo, get_author_photo,
        upsert_author_photo, AuthorPhotoSource,
    };
    use crate::books::{
        count_books, get_book, library_from_db, library_from_db_with_total, list_books,
        list_indexed_rows, sanitize_description, search_books, MAX_BOOKS_RETURNED,
    };
    use crate::browse::{list_authors, list_series};
    use crate::covers::test_helpers::CoversTempDir;
    use crate::covers::{cover_path_for, delete_cover_files_for, get_cover, write_cover_file};
    use crate::discovery::test_helpers::*;
    use crate::discovery::{get_author, get_series, get_tag_cloud, MAX_DISCOVERY_BOOKS};
    use crate::ebook::IndexedBook;
    use crate::helpers::stable_uuid;
    use crate::metadata_overrides::{
        delete_metadata_overrides, get_metadata_overrides, merge_metadata_overrides,
        upsert_metadata_overrides, write_override_cover,
    };
    use crate::palette::search_palette;
    use crate::pool::init_db;
    use crate::settings::{last_indexed_at, prune_orphan_libraries, set_settings, Settings};
    use crate::sync::test_helpers::{indexed, indexed_with_stat};
    use crate::sync::{replace_books, sync_books, SyncPlan};
    use omnibus_shared::{Contributor, EbookMetadata, Identifier, MetadataOverrides};
    use sqlx::{Row, SqlitePool};
}
