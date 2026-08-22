//! Optimistic cache application for outbox mutations: patch the replica's JSON
//! rows so the UI reflects an offline write immediately. Every helper is a
//! silent no-op when the affected list was never cached — the next online fetch
//! supersedes anyway. Also reused on online-success paths so the replica stays
//! coherent without a refetch, and by the drain's temp-to-real id remap.

use omnibus_shared::{
    Bookmark, Highlight, HighlightColor, JournalEntry, ProgressRecord, Shelf, ShelfPage,
    ShelfSummary, UpdateJournalEntry, UpdateShelfRequest, UserSummary,
};

use crate::offline::{cache, store};

/// Overwrite the cached position record for a book/format.
pub(crate) async fn progress_saved(record: &ProgressRecord) {
    let key = cache::keys::progress(&record.book_uuid, super::format_key(record.format));
    cache::put_json(&key, &Some(record.clone()));
}

// ── highlights ──────────────────────────────────────────────────────

/// Which cached highlight list contains `id` (scan of `highlights:*`).
pub(crate) async fn find_highlight_book(id: i64) -> Option<String> {
    find_book_by_id::<Highlight>("highlights:", |h| h.id == id).await
}

/// Append (or replace) a highlight in its book's cached list. Creates a
/// single-element list when none was cached, so a book annotated offline
/// still lists its new passage.
pub(crate) async fn highlight_created(highlight: &Highlight) {
    upsert_in_list(
        &cache::keys::highlights(&highlight.book_uuid),
        highlight,
        |h: &Highlight| h.id == highlight.id,
    )
    .await;
}

/// Patch a highlight's color in the cached list.
pub(crate) async fn highlight_color_changed(book_uuid: &str, id: i64, color: HighlightColor) {
    cache::mutate_json::<Vec<Highlight>, _>(&cache::keys::highlights(book_uuid), |list| {
        if let Some(h) = list.iter_mut().find(|h| h.id == id) {
            h.color = color;
        }
    })
    .await;
}

/// Patch a highlight's note in the cached list.
pub(crate) async fn highlight_note_changed(book_uuid: &str, id: i64, note: Option<String>) {
    cache::mutate_json::<Vec<Highlight>, _>(&cache::keys::highlights(book_uuid), |list| {
        if let Some(h) = list.iter_mut().find(|h| h.id == id) {
            h.note = note;
        }
    })
    .await;
}

/// Drop a highlight from the cached list.
pub(crate) async fn highlight_deleted(book_uuid: &str, id: i64) {
    cache::mutate_json::<Vec<Highlight>, _>(&cache::keys::highlights(book_uuid), |list| {
        list.retain(|h| h.id != id);
    })
    .await;
}

/// Remap a temp highlight to the server-created record.
pub(crate) async fn highlight_remapped(temp_id: i64, real: &Highlight) {
    cache::mutate_json::<Vec<Highlight>, _>(&cache::keys::highlights(&real.book_uuid), |list| {
        if let Some(h) = list.iter_mut().find(|h| h.id == temp_id) {
            *h = real.clone();
        }
    })
    .await;
}

// ── bookmarks ───────────────────────────────────────────────────────

/// Which cached bookmark list contains `id`.
pub(crate) async fn find_bookmark_book(id: i64) -> Option<String> {
    find_book_by_id::<Bookmark>("bookmarks:", |b| b.id == id).await
}

/// Append (or replace) a bookmark in its book's cached list.
pub(crate) async fn bookmark_created(bookmark: &Bookmark) {
    upsert_in_list(
        &cache::keys::bookmarks(&bookmark.book_uuid),
        bookmark,
        |b: &Bookmark| b.id == bookmark.id,
    )
    .await;
}

/// Patch a bookmark's title in the cached list.
pub(crate) async fn bookmark_renamed(book_uuid: &str, id: i64, title: Option<String>) {
    cache::mutate_json::<Vec<Bookmark>, _>(&cache::keys::bookmarks(book_uuid), |list| {
        if let Some(b) = list.iter_mut().find(|b| b.id == id) {
            b.title = title;
        }
    })
    .await;
}

/// Drop a bookmark from the cached list.
pub(crate) async fn bookmark_deleted(book_uuid: &str, id: i64) {
    cache::mutate_json::<Vec<Bookmark>, _>(&cache::keys::bookmarks(book_uuid), |list| {
        list.retain(|b| b.id != id);
    })
    .await;
}

/// Remap a temp bookmark to the server-created record.
pub(crate) async fn bookmark_remapped(temp_id: i64, real: &Bookmark) {
    cache::mutate_json::<Vec<Bookmark>, _>(&cache::keys::bookmarks(&real.book_uuid), |list| {
        if let Some(b) = list.iter_mut().find(|b| b.id == temp_id) {
            *b = real.clone();
        }
    })
    .await;
}

// ── shelves ─────────────────────────────────────────────────────────

/// Insert a shelf into the cached shelves list and cache its detail row.
pub(crate) async fn shelf_created(shelf: &Shelf) {
    upsert_in_list(
        &cache::keys::shelves(),
        &summary_of(shelf),
        |s: &ShelfSummary| s.id == shelf.id,
    )
    .await;
    cache::put_json(&cache::keys::shelf(shelf.id), shelf);
}

/// Patch a shelf in both the list and its detail row; returns the patched
/// detail when it was cached.
pub(crate) async fn shelf_updated(id: i64, req: &UpdateShelfRequest) -> Option<Shelf> {
    let key = cache::keys::shelf(id);
    let mut detail: Option<Shelf> = cache::get_json(&key).await;
    if let Some(shelf) = detail.as_mut() {
        patch_shelf(shelf, req);
        cache::put_json(&key, shelf);
    }
    cache::mutate_json::<Vec<ShelfSummary>, _>(&cache::keys::shelves(), |list| {
        if let Some(s) = list.iter_mut().find(|s| s.id == id) {
            if let Some(name) = &req.name {
                s.name = name.clone();
            }
            if let Some(v) = req.visibility {
                s.visibility = v;
            }
        }
    })
    .await;
    detail
}

/// Drop a shelf from the list plus its detail and page rows.
pub(crate) async fn shelf_deleted(id: i64) {
    cache::mutate_json::<Vec<ShelfSummary>, _>(&cache::keys::shelves(), |list| {
        list.retain(|s| s.id != id);
    })
    .await;
    if let Some(st) = store::store() {
        st.kv_delete(&cache::keys::shelf(id));
        st.kv_delete_prefix(&format!("shelf_page:{id}:"));
    }
}

/// Add books to a shelf's cached pages and bump its counts. Member rows
/// are reconstructed from the library replica when available.
pub(crate) async fn shelf_books_added(shelf_id: i64, book_uuids: &[String]) {
    bump_shelf_count(shelf_id, book_uuids.len() as i64).await;
    let Some(st) = store::store() else { return };
    let replica: Option<Vec<omnibus_shared::EbookMetadata>> =
        cache::get_json(&cache::keys::ebooks_all()).await;
    let added: Vec<omnibus_shared::EbookMetadata> = replica
        .map(|books| {
            books
                .into_iter()
                .filter(|b| {
                    b.unique_identifier
                        .as_deref()
                        .is_some_and(|u| book_uuids.iter().any(|w| w == u))
                })
                .collect()
        })
        .unwrap_or_default();
    if added.is_empty() {
        return;
    }
    for (key, _) in st.kv_prefix(&format!("shelf_page:{shelf_id}:")).await {
        cache::mutate_json::<ShelfPage, _>(&key, |page| {
            for book in &added {
                let uuid = book.unique_identifier.as_deref().unwrap_or_default();
                let exists = page
                    .books
                    .iter()
                    .any(|b| b.unique_identifier.as_deref() == Some(uuid));
                if !exists {
                    page.books.push(book.clone());
                }
            }
        })
        .await;
    }
}

/// Remove a book from a shelf's cached pages and drop its counts.
pub(crate) async fn shelf_book_removed(shelf_id: i64, book_uuid: &str) {
    bump_shelf_count(shelf_id, -1).await;
    let Some(st) = store::store() else { return };
    for (key, _) in st.kv_prefix(&format!("shelf_page:{shelf_id}:")).await {
        cache::mutate_json::<ShelfPage, _>(&key, |page| {
            page.books
                .retain(|b| b.unique_identifier.as_deref() != Some(book_uuid));
        })
        .await;
    }
}

/// Remap a temp shelf to the server-created record: fix the list entry,
/// move the detail row, and rename any cached page keys.
pub(crate) async fn shelf_remapped(temp_id: i64, real: &Shelf) {
    cache::mutate_json::<Vec<ShelfSummary>, _>(&cache::keys::shelves(), |list| {
        if let Some(s) = list.iter_mut().find(|s| s.id == temp_id) {
            *s = summary_of(real);
        }
    })
    .await;
    if let Some(st) = store::store() {
        st.kv_delete(&cache::keys::shelf(temp_id));
        for (key, _) in st.kv_prefix(&format!("shelf_page:{temp_id}:")).await {
            let suffix = key
                .strip_prefix(&format!("shelf_page:{temp_id}:"))
                .unwrap_or_default()
                .to_string();
            st.kv_rename(&key, &format!("shelf_page:{}:{suffix}", real.id));
        }
    }
    cache::put_json(&cache::keys::shelf(real.id), real);
}

// ── journals ────────────────────────────────────────────────────────

/// Which cached journal list contains entry `id`.
pub(crate) async fn find_journal_book(id: i64) -> Option<String> {
    find_book_by_id::<JournalEntry>("journals:", |e| e.id == id).await
}

/// Prepend a journal entry to its book's cached list (newest first, the
/// server's ordering).
pub(crate) async fn journal_created(entry: &JournalEntry) {
    let key = cache::keys::journals(&entry.book_uuid);
    let mut list: Vec<JournalEntry> = cache::get_json(&key).await.unwrap_or_default();
    list.retain(|e| e.id != entry.id);
    list.insert(0, entry.clone());
    cache::put_json(&key, &list);
}

/// Patch a journal entry in the cached list; returns the patched entry.
pub(crate) async fn journal_updated(
    book_uuid: &str,
    id: i64,
    input: &UpdateJournalEntry,
) -> Option<JournalEntry> {
    let key = cache::keys::journals(book_uuid);
    let mut list: Vec<JournalEntry> = cache::get_json(&key).await?;
    let entry = list.iter_mut().find(|e| e.id == id)?;
    entry.body_md = input.body_md.clone();
    entry.body_html = super::fallback_html(&input.body_md);
    entry.progress = input.progress;
    if let Some(status) = input.status {
        entry.status = status;
    }
    entry.updated_at = store::now_secs();
    let patched = entry.clone();
    cache::put_json(&key, &list);
    Some(patched)
}

/// Drop a journal entry from the cached list.
pub(crate) async fn journal_deleted(book_uuid: &str, id: i64) {
    cache::mutate_json::<Vec<JournalEntry>, _>(&cache::keys::journals(book_uuid), |list| {
        list.retain(|e| e.id != id);
    })
    .await;
}

/// Remap a temp journal entry to the server-created record.
pub(crate) async fn journal_remapped(temp_id: i64, real: &JournalEntry) {
    cache::mutate_json::<Vec<JournalEntry>, _>(&cache::keys::journals(&real.book_uuid), |list| {
        if let Some(e) = list.iter_mut().find(|e| e.id == temp_id) {
            *e = real.clone();
        }
    })
    .await;
}

// ── account ─────────────────────────────────────────────────────────

/// Patch the cached account summary's Kindle email.
pub(crate) async fn kindle_email_changed(email: &Option<String>) {
    let email = email.clone();
    cache::mutate_json::<UserSummary, _>(&cache::keys::me(), |me| {
        me.kindle_email = email;
    })
    .await;
}

// ── shared plumbing ─────────────────────────────────────────────────

fn summary_of(shelf: &Shelf) -> ShelfSummary {
    ShelfSummary {
        id: shelf.id,
        owner_user_id: shelf.owner_user_id,
        owner_username: shelf.owner_username.clone(),
        kind: shelf.kind,
        name: shelf.name.clone(),
        visibility: shelf.visibility,
        accent: shelf.accent.clone(),
        book_count: shelf.book_count,
        // The optimistic replica has no thumbnail knowledge; an empty mosaic
        // is the honest offline rendering until the next real fetch.
        cover_uuids: Vec::new(),
    }
}

fn patch_shelf(shelf: &mut Shelf, req: &UpdateShelfRequest) {
    if let Some(name) = &req.name {
        shelf.name = name.clone();
    }
    if req.description.is_some() {
        shelf.description = req.description.clone();
    }
    if let Some(v) = req.visibility {
        shelf.visibility = v;
    }
    if let Some(mode) = req.match_mode {
        shelf.match_mode = Some(mode);
    }
    if let Some(rules) = &req.rules {
        shelf.rules = rules.clone();
    }
}

async fn bump_shelf_count(shelf_id: i64, delta: i64) {
    cache::mutate_json::<Vec<ShelfSummary>, _>(&cache::keys::shelves(), |list| {
        if let Some(s) = list.iter_mut().find(|s| s.id == shelf_id) {
            s.book_count = (s.book_count + delta).max(0);
        }
    })
    .await;
    cache::mutate_json::<Shelf, _>(&cache::keys::shelf(shelf_id), |shelf| {
        shelf.book_count = (shelf.book_count + delta).max(0);
    })
    .await;
}

/// Append-or-replace one record in a cached list, creating the list when
/// absent.
async fn upsert_in_list<T>(key: &str, record: &T, matches: impl Fn(&T) -> bool)
where
    T: serde::Serialize + serde::de::DeserializeOwned + Clone,
{
    let mut list: Vec<T> = cache::get_json(key).await.unwrap_or_default();
    list.retain(|item| !matches(item));
    list.push(record.clone());
    cache::put_json(key, &list);
}

/// Scan every cached list under `prefix` for an entry matching `pred`,
/// returning the book uuid embedded in the key. Bounded by the number of
/// books whose lists were ever cached.
async fn find_book_by_id<T: serde::de::DeserializeOwned>(
    prefix: &str,
    pred: impl Fn(&T) -> bool,
) -> Option<String> {
    let st = store::store()?;
    for (key, payload) in st.kv_prefix(prefix).await {
        let Ok(list) = serde_json::from_str::<Vec<T>>(&payload) else {
            continue;
        };
        if list.iter().any(&pred) {
            return key.strip_prefix(prefix).map(str::to_string);
        }
    }
    None
}
