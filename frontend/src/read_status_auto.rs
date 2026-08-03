//! Automatic read-status transitions driven by the readers and the audio
//! players: starting an `Unread` book marks it `Reading`, and reaching its
//! end marks it `Finished`. Pure decision logic, the hook the EPUB reader and
//! the CBZ pager mount, and the one-shot [`apply_auto_read_status`] the
//! players call from their media-event callbacks.

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

/// Apply [`auto_transition`] once, outside a component.
///
/// The readers observe `at_end` as reactive state and drive it through
/// [`use_auto_read_status`]. The audio players have no such signal — they are
/// driven by media events fired from JS — so they call this directly: `false`
/// on the first play of a book, `true` when the last part reaches its end.
///
/// Best-effort like the readers' writes, and for the same reason a failed
/// *fetch* decides nothing: writing `Reading` over an unfetched `Finished`
/// would be a downgrade. The next event retries.
pub async fn apply_auto_read_status(server_url: &str, uuid: &str, at_end: bool) {
    let Ok(stored) = data::get_read_status(server_url, uuid).await else {
        return;
    };
    let Some(next) = auto_transition(stored.map(|r| r.status).unwrap_or_default(), at_end) else {
        return;
    };
    let _ = data::set_read_status(
        server_url,
        SetReadStatus {
            book_uuid: uuid.to_string(),
            status: next,
        },
    )
    .await;
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
