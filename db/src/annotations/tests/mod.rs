//! Unit tests for the `annotations` module, split by channel into the
//! sibling modules below; the book and user seeding fixtures they share
//! live here.

mod client_id;
mod highlights;
mod kobo;
mod placement;

use omnibus_shared::EbookMetadata;

use super::*;
use crate::replace_books;

async fn seed(pool: &SqlitePool, library: &str, title: &str) -> (i64, String) {
    replace_books(
        pool,
        library,
        vec![crate::ebook::IndexedBook {
            metadata: EbookMetadata {
                filename: format!("{title}.epub").to_lowercase(),
                title: Some(title.to_string()),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
            word_count: None,
        }],
    )
    .await
    .expect("seed book");
    let books = crate::list_books(pool, library).await.unwrap();
    let book = books
        .into_iter()
        .find(|b| b.title.as_deref() == Some(title))
        .unwrap();
    (book.id, book.unique_identifier.clone().unwrap())
}

async fn seed_user(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash, is_admin, can_upload, can_edit, can_download)
         VALUES (?, '!x', 0, 0, 0, 1) RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Give a book's EPUB the derived structure the anchor index reads: four
/// equal spine documents and a TOC entry naming each of the last three.
/// Written directly rather than extracted, so the test needs no archive.
async fn seed_epub_structure(pool: &SqlitePool, book_id: i64) {
    let file_id: i64 = sqlx::query_scalar(
        "SELECT id FROM book_files WHERE book_id = ? AND format = 'EPUB' ORDER BY id LIMIT 1",
    )
    .bind(book_id)
    .fetch_one(pool)
    .await
    .unwrap();
    for spine_index in 0..4i64 {
        sqlx::query(
            "INSERT INTO epub_spine_stats (book_file_id, spine_index, href, visible_chars, chars_before)
             VALUES (?, ?, ?, 1000, ?)",
        )
        .bind(file_id)
        .bind(spine_index)
        .bind(format!("c{spine_index}.xhtml"))
        .bind(spine_index * 1000)
        .execute(pool)
        .await
        .unwrap();
    }
    for (ordinal, title, spine_index) in [(0i64, "One", 1i64), (1, "Two", 2), (2, "Three", 3)] {
        sqlx::query(
            "INSERT INTO ebook_chapters (book_file_id, ordinal, title, href, spine_index, start_chars)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(file_id)
        .bind(ordinal)
        .bind(title)
        .bind(format!("c{spine_index}.xhtml"))
        .bind(spine_index)
        .bind(spine_index * 1000)
        .execute(pool)
        .await
        .unwrap();
    }
}
