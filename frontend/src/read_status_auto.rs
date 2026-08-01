//! Automatic read-status transitions driven by the readers: opening an
//! `Unread` book marks it `Reading`, and reaching the end marks it
//! `Finished` — the reader-side sibling of the iOS audio player's
//! finish-on-playback-end. Pure decision logic plus the hook the EPUB
//! reader and the CBZ pager both mount.

use dioxus::prelude::*;
use omnibus_shared::{ReadStatus, SetReadStatus};

use crate::data;

#[cfg(test)]
mod tests;

/// The status to write for a book whose stored state is `current`, observed
/// either at the reader's end position (`at_end`) or merely open. `None`
/// means no write: opening never downgrades a `Finished` book back to
/// `Reading`, and re-reaching the end of a `Finished` book is a no-op.
pub fn auto_transition(current: ReadStatus, at_end: bool) -> Option<ReadStatus> {
    if at_end {
        (current != ReadStatus::Finished).then_some(ReadStatus::Finished)
    } else {
        (current == ReadStatus::Unread).then_some(ReadStatus::Reading)
    }
}

/// Drive [`auto_transition`] for the book open in a reader: fetch the stored
/// status once per book, then re-apply whenever the status or the reader's
/// `at_end` position changes. Writes are best-effort like the readers'
/// progress saves — a failed write must not interrupt reading, and the next
/// open retries. Call unconditionally from the page (rule 07); on SSR the
/// effects never fire and this is a no-op.
pub fn use_auto_read_status(uuid: String, server_url: String, at_end: Memo<bool>) {
    // The fetched status, keyed by the uuid the fetch answered for: the
    // write effect reads the pair together, so it can never decide against
    // — or write to — a book the fetch didn't describe. `None` until the
    // fetch lands, and stays `None` when it fails, so a transition is never
    // decided against a guessed status (writing `Reading` over an unfetched
    // `Finished` would be a downgrade).
    let mut status: Signal<Option<(String, ReadStatus)>> = use_signal(|| None);
    // Monotonic fetch guard (`load_seq` in `pages/book_detail/read_status.rs`
    // is the model): a route-param book swap reuses the mounted page, so a
    // slow response for the previous book must not land as the current one's.
    let mut load_seq = use_signal(|| 0u64);

    {
        let server_url = server_url.clone();
        use_effect(use_reactive!(|uuid| {
            let my_load = *load_seq.peek() + 1;
            load_seq.set(my_load);
            status.set(None);
            let server_url = server_url.clone();
            spawn(async move {
                if let Ok(stored) = data::get_read_status(&server_url, &uuid).await {
                    if *load_seq.peek() == my_load {
                        let fetched = stored.map(|r| r.status).unwrap_or_default();
                        status.set(Some((uuid, fetched)));
                    }
                }
            });
        }));
    }

    use_effect(move || {
        let Some((book_uuid, current)) = status() else {
            return;
        };
        let Some(next) = auto_transition(current, at_end()) else {
            return;
        };
        // Optimistic: move the signal first so the re-run settles on `None`
        // instead of repeating the write while the POST is in flight.
        status.set(Some((book_uuid.clone(), next)));
        let server_url = server_url.clone();
        let update = SetReadStatus {
            book_uuid,
            status: next,
        };
        spawn(async move {
            let _ = data::set_read_status(&server_url, update).await;
        });
    });
}
