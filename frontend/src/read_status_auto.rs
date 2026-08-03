//! Automatic read-status transitions driven by the readers and the audio
//! players: starting an `Unread` book marks it `Reading`, and reaching its
//! end marks it `Finished`. Pure decision logic, the hook the EPUB reader and
//! the CBZ pager mount, and the one-shot [`apply_auto_read_status`] the
//! players call from their media-event callbacks.

use dioxus::prelude::*;
use omnibus_shared::{ReadStatus, ReadStatusRecord, SetReadStatus};

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

/// Apply a status fetch's outcome to `status`, dropping it if a later
/// navigation has already bumped `load_seq` past `my_load`. Same guard shape
/// as `apply_bootstrap_outcome` in `pages/comic_reader.rs` — pulled out of
/// the fetch effect so the guard is exercised directly, without a network
/// round trip.
fn apply_fetched_status(
    fetched: Option<ReadStatusRecord>,
    my_load: u64,
    load_seq: Signal<u64>,
    uuid: String,
    mut status: Signal<Option<(String, ReadStatus)>>,
) {
    if *load_seq.peek() != my_load {
        return;
    }
    let fetched = fetched.map(|r| r.status).unwrap_or_default();
    status.set(Some((uuid, fetched)));
}

/// Decide the write effect's next move: `None` when nothing has been
/// fetched yet or [`auto_transition`] has nothing to do (including the
/// never-downgrade case — opening a `Finished` book with `at_end: false`).
/// On `Some`, `status` has already been moved to the optimistic next value
/// (so a re-run settles on it instead of repeating the write while the POST
/// is in flight) and the returned payload is what the caller persists.
fn decide_transition_write(
    mut status: Signal<Option<(String, ReadStatus)>>,
    at_end: bool,
) -> Option<SetReadStatus> {
    let (book_uuid, current) = status()?;
    let next = auto_transition(current, at_end)?;
    status.set(Some((book_uuid.clone(), next)));
    Some(SetReadStatus {
        book_uuid,
        status: next,
    })
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
            let uuid_for_apply = uuid.clone();
            spawn(async move {
                if let Ok(stored) = data::get_read_status(&server_url, &uuid).await {
                    apply_fetched_status(stored, my_load, load_seq, uuid_for_apply, status);
                }
            });
        }));
    }

    use_effect(move || {
        let Some(update) = decide_transition_write(status, at_end()) else {
            return;
        };
        let server_url = server_url.clone();
        spawn(async move {
            let _ = data::set_read_status(&server_url, update).await;
        });
    });
}
