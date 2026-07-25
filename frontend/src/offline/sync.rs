//! Connectivity state and the sync engine. `NetState` rides a
//! `tokio::sync::watch` channel (the `token_store::subscribe` house
//! pattern) so background threads never touch Dioxus signals; UI
//! components subscribe inside a `use_future`. The drain replays the
//! outbox in enqueue order with last-write-wins semantics — the project's
//! committed conflict stance (docs/roadmap/2-1-progress-sync.md) — and a
//! stale guard on progress writes so a queued position can't clobber a
//! newer one written by another device while this one was offline.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};

use tokio::sync::watch;

use crate::data::{self, DataError};

use super::{outbox, outbox::Op, store};

/// Re-probe `/api/_health` this often while offline.
const PROBE_INTERVAL_SECS: u64 = 15;

/// Background sync cadence while online and running.
const SYNC_TICK_SECS: u64 = 60;

/// Connectivity + queue snapshot for the UI (offline pill, sync row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetState {
    /// `false` after a connectivity-class failure, until a request or the
    /// health probe succeeds again.
    pub online: bool,
    /// Queued (not yet drained) mutations.
    pub pending_ops: usize,
    /// Ops dropped this session because the server permanently rejected
    /// them (4xx) — surfaced as "N changes couldn't sync".
    pub dropped_ops: usize,
    /// Ops dropped this session after exhausting their retry budget on
    /// repeated server errors (5xx) — distinct from `dropped_ops`: these
    /// were never permanently rejected, they just never got a good answer
    /// after [`MAX_SERVER_ERROR_ATTEMPTS`] tries. Surfaced next to it.
    pub stuck_ops: usize,
}

impl NetState {
    fn initial() -> Self {
        NetState {
            online: true,
            pending_ops: 0,
            dropped_ops: 0,
            stuck_ops: 0,
        }
    }
}

fn channel() -> &'static (watch::Sender<NetState>, watch::Receiver<NetState>) {
    static CH: OnceLock<(watch::Sender<NetState>, watch::Receiver<NetState>)> = OnceLock::new();
    CH.get_or_init(|| watch::channel(NetState::initial()))
}

/// Subscribe to connectivity/queue changes. Initial value reflects the
/// state at subscribe time.
pub fn subscribe() -> watch::Receiver<NetState> {
    channel().0.subscribe()
}

/// Current snapshot.
pub fn state() -> NetState {
    *channel().0.borrow()
}

/// `true` while the last classified request failed with a
/// connectivity-class error and no probe has succeeded since.
pub fn is_offline() -> bool {
    !state().online
}

fn update(f: impl FnOnce(&mut NetState)) {
    channel().0.send_if_modified(|s| {
        let before = *s;
        f(s);
        before != *s
    });
}

/// Record a successful round-trip: flips to online and kicks a drain when
/// anything is queued.
pub(crate) fn note_online() {
    let was_offline = is_offline();
    update(|s| s.online = true);
    if was_offline || state().pending_ops > 0 {
        request_drain();
    }
}

/// Record a connectivity-class failure: flips to offline (the probe loop
/// takes over from here).
pub(crate) fn note_offline() {
    update(|s| s.online = false);
}

/// Refresh the queued-op count on the channel.
pub(crate) async fn refresh_pending() {
    let count = match store::store() {
        Some(store) => store.ops_count().await,
        None => 0,
    };
    update(|s| s.pending_ops = count);
}

/// Record permanently-rejected ops for the account-screen notice.
pub(crate) fn note_dropped(n: usize) {
    if n > 0 {
        update(|s| s.dropped_ops += n);
    }
}

/// Record ops dropped after exhausting their server-error retry budget.
pub(crate) fn note_stuck(n: usize) {
    if n > 0 {
        update(|s| s.stuck_ops += n);
    }
}

/// `true` when `e` means "the server could not be reached" — the only class
/// of failure that triggers cache fallback or queueing. Response-body decode
/// failures also surface as `DataError::Network` on mobile
/// (`reqwest::Error::is_decode`), and those mean the server *was* reachable,
/// so they are explicitly excluded.
pub(crate) fn is_offline_error(e: &DataError) -> bool {
    match e {
        DataError::Network(re) => !re.is_decode(),
        // A fast-fail precheck skipped the network because we already knew
        // we were offline — same class as a failed connect.
        DataError::Offline => true,
        _ => false,
    }
}

/// Server base URL for background work (drain, probe) that runs outside any
/// Dioxus scope. Kept fresh by [`use_offline_runtime`].
fn server_url_cell() -> &'static RwLock<String> {
    static CELL: OnceLock<RwLock<String>> = OnceLock::new();
    CELL.get_or_init(|| RwLock::new(String::new()))
}

/// Record the current server base URL for the drain/probe loops.
pub(crate) fn set_server_url(url: &str) {
    if let Ok(mut guard) = server_url_cell().write() {
        if *guard != url {
            *guard = url.to_string();
        }
    }
    super::media::set_upstream(url);
}

fn server_url() -> Option<String> {
    let guard = server_url_cell().read().ok()?;
    (!guard.is_empty()).then(|| guard.clone())
}

/// One background maintenance pass (Kindle-style sync): drain the outbox,
/// refresh the full-library replica (TTL-gated), and re-warm the resume
/// rail. Never blocks UI — it runs on the offline runtime loop, and every
/// read it issues is cache-first.
async fn background_tick() {
    if is_offline() {
        return;
    }
    let Some(url) = server_url() else { return };
    request_drain();
    super::replica::ensure_fresh(url.clone());
    // Cache-first, so this returns instantly and revalidates when stale —
    // keeping "pick up where you left off" fresh without a page visit.
    let _ = data::recent_progress(&url, 1).await;
}

/// App-root hook: records the reactive server URL for background work and
/// runs the offline runtime loop — probe-when-offline, drain triggers, and
/// the periodic background sync tick while online.
/// The non-mobile stub in `lib.rs` keeps call sites gate-free
/// (`use_mobile_viewport_fix` pattern).
pub fn use_offline_runtime() {
    use dioxus::prelude::*;

    let url_signal = crate::contexts::use_server_url_signal();
    use_effect(move || {
        set_server_url(&url_signal());
    });

    // One app-lifetime background loop: kick an initial drain (cold-start
    // queue from a previous session), then tick-while-online / probe-while-
    // offline.
    use_future(move || async move {
        request_drain();
        let mut rx = subscribe();
        // Boot tick once the server URL is known, so the replica refresh
        // doesn't depend on the user visiting the landing page first.
        let mut booted = false;
        loop {
            let snapshot = *rx.borrow_and_update();
            if snapshot.online {
                if !booted && server_url().is_some() {
                    booted = true;
                    background_tick().await;
                }
                // Idle until the state changes (a failure flips us offline;
                // pending-count changes are informational) or the next
                // periodic sync tick fires.
                tokio::select! {
                    changed = rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_secs(SYNC_TICK_SECS)) => {
                        background_tick().await;
                    }
                }
                continue;
            }
            // Offline: probe on an interval until the server answers.
            tokio::time::sleep(std::time::Duration::from_secs(PROBE_INTERVAL_SECS)).await;
            if let Some(url) = server_url() {
                if data::check_server(&url).await.is_ok() {
                    note_online();
                    // Re-read + revalidate visible pages right away instead
                    // of waiting out the next tick.
                    background_tick().await;
                    super::cache::notify_changed();
                }
            }
        }
    });
}

/// Network-first write policy: try the server; on a connectivity-class
/// failure (or while already known-offline, skipping a doomed connect
/// wait), hand the mutation to `queue` — the outbox closure that enqueues,
/// optimistically applies, and synthesizes the return value. `queue`
/// yielding `None` means the offline store is unavailable; the original
/// behavior (surface the error) applies.
pub(crate) async fn write_through<T, OF, Q, QF>(
    online: impl FnOnce() -> OF,
    queue: Q,
) -> Result<T, DataError>
where
    OF: std::future::Future<Output = Result<T, DataError>>,
    Q: Fn() -> QF,
    QF: std::future::Future<Output = Option<T>>,
{
    if is_offline() {
        if let Some(v) = queue().await {
            return Ok(v);
        }
    }
    match online().await {
        Ok(v) => {
            note_online();
            Ok(v)
        }
        Err(e) if is_offline_error(&e) => {
            note_offline();
            match queue().await {
                Some(v) => Ok(v),
                None => Err(e),
            }
        }
        Err(e) => {
            note_online();
            Err(e)
        }
    }
}

/// Serialized drain claim: `request_drain` is called from many places (any
/// successful request, the probe, boot) but only one drain runs at a time.
static DRAIN_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Ask for an outbox drain on the offline runtime. No-op while one is
/// already running or when the runtime/store is unavailable.
pub(crate) fn request_drain() {
    if store::store().is_none() {
        return;
    }
    if DRAIN_ACTIVE.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = super::media::spawn_on_runtime(async {
        drain().await;
        DRAIN_ACTIVE.store(false, Ordering::SeqCst);
    });
    if !spawned {
        DRAIN_ACTIVE.store(false, Ordering::SeqCst);
    }
}

/// Per-op drain outcome (see [`execute_op`]).
enum Outcome {
    /// Replayed (or made moot) — delete the op row.
    Done,
    /// Permanently rejected by the server (4xx) — delete and count as
    /// dropped.
    Rejected,
    /// This op's target errored (5xx) or its response failed to decode.
    /// Unlike [`Outcome::Halt`], this does NOT mean the device is offline —
    /// keep the op queued, bump its attempt count, and keep draining the
    /// rest of the queue.
    ServerError,
    /// A [`Outcome::ServerError`] that has exhausted
    /// [`MAX_SERVER_ERROR_ATTEMPTS`] retries — delete and count as stuck
    /// rather than let it wedge every future drain forever.
    Stuck,
    /// Connectivity failure — keep the op, flip the device offline, and
    /// stop the drain (every other queued op would fail the same way).
    Halt,
    /// Session expired — keep every op for after re-login, stop the drain.
    HaltAuth,
}

/// Consecutive server-error attempts an op gets before it's treated as
/// permanently stuck and dropped. Drains are already externally triggered
/// (reconnect, periodic tick, manual probe) rather than tight-looped, so a
/// small fixed budget is enough to distinguish "flaky, retry it" from
/// "this op will never succeed" without needing real exponential backoff.
const MAX_SERVER_ERROR_ATTEMPTS: i64 = 5;

/// Replay the outbox in enqueue order. One pass per invocation; ops that
/// arrive mid-drain are picked up by the next trigger.
async fn drain() {
    let Some(st) = store::store() else { return };
    let Some(url) = server_url() else {
        refresh_pending().await;
        return;
    };
    let mut dropped = 0usize;
    let mut stuck = 0usize;
    let mut resolved = 0usize;
    for row in st.ops_list().await {
        let Ok(op) = serde_json::from_str::<Op>(&row.payload) else {
            // Undecodable payload (schema drift) — drop rather than wedge
            // the queue forever.
            st.ops_delete_many(vec![row.id]).await;
            dropped += 1;
            continue;
        };
        let id = row.id;
        // A ServerError that has exhausted its retry budget is promoted to
        // Stuck here (rather than inside `classify`), since only the drain
        // loop knows the op's attempt history.
        let outcome = match execute_op(&url, op).await {
            Outcome::ServerError if row.attempts + 1 >= MAX_SERVER_ERROR_ATTEMPTS => Outcome::Stuck,
            other => other,
        };
        match outcome {
            Outcome::Done => {
                st.ops_delete_many(vec![id]).await;
                resolved += 1;
            }
            Outcome::Rejected => {
                st.ops_delete_many(vec![id]).await;
                dropped += 1;
                resolved += 1;
            }
            // This op's own target is unhappy — not the device's
            // connectivity — so keep going rather than break the loop and
            // wedge every op queued behind it.
            Outcome::ServerError => {
                st.ops_bump_attempts(id);
            }
            Outcome::Stuck => {
                st.ops_delete_many(vec![id]).await;
                stuck += 1;
                resolved += 1;
            }
            Outcome::Halt => {
                st.ops_bump_attempts(id);
                note_offline();
                break;
            }
            Outcome::HaltAuth => break,
        }
    }
    note_dropped(dropped);
    note_stuck(stuck);
    refresh_pending().await;
    if let Some(st) = store::store() {
        st.meta_put("last_sync_at", store::now_secs().to_string());
    }
    if resolved > 0 {
        // Replayed ops rewrote cache rows (server records, temp-id remaps)
        // — nudge open pages to re-read, and, with the queue now shorter,
        // let their refetches revalidate against the server again.
        super::cache::notify_changed();
    }
}

/// Classify a replay error. `deletion` relaxes 404 to success — the target
/// vanished server-side, which is what the queued delete wanted anyway.
fn classify(e: &DataError, target_may_be_gone: bool) -> Outcome {
    match e {
        DataError::Unauthorized => Outcome::HaltAuth,
        DataError::Http { status: 404, .. } if target_may_be_gone => Outcome::Done,
        DataError::Http { status, .. } if *status >= 500 => Outcome::ServerError,
        DataError::Http { .. } => Outcome::Rejected,
        e if is_offline_error(e) => Outcome::Halt,
        // Decode-class network errors: the server acted but the response
        // was unreadable; retrying won't improve it.
        _ => Outcome::Rejected,
    }
}

fn ok_or<T>(result: Result<T, DataError>, target_may_be_gone: bool) -> (Option<T>, Outcome) {
    match result {
        Ok(v) => (Some(v), Outcome::Done),
        Err(e) => (None, classify(&e, target_may_be_gone)),
    }
}

/// Replay one op against the server, applying remaps / cache refreshes on
/// success.
async fn execute_op(url: &str, op: Op) -> Outcome {
    match op {
        Op::SaveProgress {
            update,
            captured_at,
        } => {
            // LWW stale guard: if another device wrote a *newer* position
            // while this one was offline, keep the server's copy.
            if let Ok(Some(server)) =
                data::get_progress_online(url, &update.book_uuid, update.format).await
            {
                if server.updated_at > captured_at {
                    outbox::apply::progress_saved(&server).await;
                    return Outcome::Done;
                }
            }
            let (record, outcome) = ok_or(data::save_progress_online(url, update).await, false);
            if let Some(record) = record {
                outbox::apply::progress_saved(&record).await;
            }
            outcome
        }
        Op::SetPlaybackRate { uuid, update } => {
            let (record, outcome) = ok_or(
                data::set_playback_rate_online(url, &uuid, update).await,
                false,
            );
            if let Some(record) = record {
                super::cache::put_json(&super::cache::keys::playback_rate(&uuid), &Some(record));
            }
            outcome
        }
        Op::RecordSessions { reports } => {
            ok_or(data::record_sessions_online(url, reports).await, false).1
        }
        Op::SetRating { update } => {
            let uuid = update.book_uuid.clone();
            let (record, outcome) = ok_or(data::set_rating_online(url, update).await, false);
            if let Some(record) = record {
                super::cache::put_json(&super::cache::keys::rating(&uuid), &Some(record));
            }
            outcome
        }
        Op::ClearRating { uuid } => ok_or(data::clear_rating_online(url, &uuid).await, true).1,
        Op::CreateHighlight { temp_id, input } => {
            let (created, outcome) = ok_or(data::create_highlight_online(url, input).await, false);
            if let Some(created) = created {
                outbox::apply::highlight_remapped(temp_id, &created).await;
                outbox::remap_queued(temp_id, created.id).await;
            }
            outcome
        }
        Op::UpdateHighlightColor { id, .. } if id < 0 => Outcome::Done,
        Op::UpdateHighlightColor { id, color, .. } => {
            ok_or(
                data::update_highlight_color_online(url, id, color).await,
                true,
            )
            .1
        }
        Op::UpdateHighlightNote { id, .. } if id < 0 => Outcome::Done,
        Op::UpdateHighlightNote { id, note, .. } => {
            ok_or(
                data::update_highlight_note_online(url, id, note).await,
                true,
            )
            .1
        }
        Op::DeleteHighlight { id, .. } if id < 0 => Outcome::Done,
        Op::DeleteHighlight { id, .. } => {
            ok_or(data::delete_highlight_online(url, id).await, true).1
        }
        Op::CreateBookmark { temp_id, input } => {
            let (created, outcome) = ok_or(data::create_bookmark_online(url, input).await, false);
            if let Some(created) = created {
                outbox::apply::bookmark_remapped(temp_id, &created).await;
                outbox::remap_queued(temp_id, created.id).await;
            }
            outcome
        }
        Op::UpdateBookmark { id, .. } if id < 0 => Outcome::Done,
        Op::UpdateBookmark { id, title, .. } => {
            ok_or(data::update_bookmark_online(url, id, title).await, true).1
        }
        Op::DeleteBookmark { id, .. } if id < 0 => Outcome::Done,
        Op::DeleteBookmark { id, .. } => ok_or(data::delete_bookmark_online(url, id).await, true).1,
        Op::CreateShelf { temp_id, req } => {
            let (created, outcome) = ok_or(data::create_shelf_online(url, req).await, false);
            if let Some(created) = created {
                outbox::apply::shelf_remapped(temp_id, &created).await;
                outbox::remap_queued(temp_id, created.id).await;
            }
            outcome
        }
        Op::UpdateShelf { id, .. } if id < 0 => Outcome::Done,
        Op::UpdateShelf { id, req } => ok_or(data::update_shelf_online(url, id, req).await, true).1,
        Op::DeleteShelf { id } if id < 0 => Outcome::Done,
        Op::DeleteShelf { id } => ok_or(data::delete_shelf_online(url, id).await, true).1,
        Op::AddShelfBooks { shelf_id, .. } if shelf_id < 0 => Outcome::Done,
        Op::AddShelfBooks {
            shelf_id,
            book_uuids,
        } => {
            ok_or(
                data::add_shelf_books_online(url, shelf_id, book_uuids).await,
                true,
            )
            .1
        }
        Op::RemoveShelfBook { shelf_id, .. } if shelf_id < 0 => Outcome::Done,
        Op::RemoveShelfBook {
            shelf_id,
            book_uuid,
        } => {
            ok_or(
                data::remove_shelf_book_online(url, shelf_id, &book_uuid).await,
                true,
            )
            .1
        }
        Op::CreateJournal { temp_id, input } => {
            let (created, outcome) =
                ok_or(data::create_journal_entry_online(url, input).await, false);
            if let Some(created) = created {
                outbox::apply::journal_remapped(temp_id, &created).await;
                outbox::remap_queued(temp_id, created.id).await;
            }
            outcome
        }
        Op::UpdateJournal { id, .. } if id < 0 => Outcome::Done,
        Op::UpdateJournal { id, input, .. } => {
            let (updated, outcome) = ok_or(
                data::update_journal_entry_online(url, id, input).await,
                true,
            );
            if let Some(updated) = updated {
                outbox::apply::journal_remapped(id, &updated).await;
            }
            outcome
        }
        Op::DeleteJournal { id, .. } if id < 0 => Outcome::Done,
        Op::DeleteJournal { id, .. } => {
            ok_or(data::delete_journal_entry_online(url, id).await, true).1
        }
        Op::SetKindleEmail { email } => {
            ok_or(data::set_kindle_email_online(url, email).await, true).1
        }
    }
}

/// Serializes tests (across offline test modules) that assert on the
/// process-global `NetState` — `read_through` flips it as a side effect, so
/// unsynchronized parallel tests would race each other's assertions.
#[cfg(test)]
pub(crate) fn test_state_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
mod tests;
