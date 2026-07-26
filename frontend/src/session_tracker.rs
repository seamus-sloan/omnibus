//! Reading/listening session capture feeding the `/stats` aggregates: a
//! pure segment state machine ([`SessionTracker`]) plus the two hooks that
//! drive it — readers count mounted-and-visible wall-clock time, players
//! count play→pause spans — flushing [`SessionReport`]s to the session
//! tables (web via `navigator.sendBeacon`, mobile via the REST layer).

use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;
use omnibus_shared::{ProgressFormat, SessionReport};

#[cfg(test)]
mod tests;

/// Interval between heartbeat flushes of a still-running segment, so a
/// killed tab/app loses at most this much recorded time.
pub const HEARTBEAT_SECS: u64 = 60;

/// Segments shorter than this are dropped — a bounce into and straight
/// back out of a book is noise, not a reading session.
pub const MIN_SESSION_SECS: i64 = 5;

/// Where the tracker is in a book's lifecycle. `Paused` remembers the book
/// so a visibility resume can restart the segment without re-asking the
/// surface which book is open.
enum Phase {
    Idle,
    Paused { uuid: String },
    Active { uuid: String, started_at: i64 },
}

/// Pure active-segment state machine. All transitions take `now` (unix
/// secs) injected by the caller so the machine stays clock-free and
/// testable; transitions that end a segment return the [`SessionReport`]
/// to send (or `None` when the segment was too short to keep).
pub struct SessionTracker {
    format: ProgressFormat,
    phase: Phase,
}

impl SessionTracker {
    /// A tracker with no active segment, reporting sessions as `format`.
    pub fn new(format: ProgressFormat) -> Self {
        Self {
            format,
            phase: Phase::Idle,
        }
    }

    /// The surface says `uuid` is in front and counting. Starting the same
    /// book again is a no-op; switching books ends the old segment and
    /// returns its report.
    pub fn start(&mut self, uuid: &str, now: i64) -> Option<SessionReport> {
        match &self.phase {
            Phase::Active { uuid: cur, .. } if cur == uuid => None,
            Phase::Active { .. } => {
                let report = self.take_report(now);
                self.phase = Phase::Active {
                    uuid: uuid.to_string(),
                    started_at: now,
                };
                report
            }
            Phase::Idle | Phase::Paused { .. } => {
                self.phase = Phase::Active {
                    uuid: uuid.to_string(),
                    started_at: now,
                };
                None
            }
        }
    }

    /// End the segment and forget the book (player paused, surface torn
    /// down). Returns the report for the ended segment, if any.
    pub fn stop(&mut self, now: i64) -> Option<SessionReport> {
        let report = self.take_report(now);
        self.phase = Phase::Idle;
        report
    }

    /// End the segment but remember the book (tab hidden), so [`resume`]
    /// can restart it without the caller knowing the uuid.
    ///
    /// [`resume`]: SessionTracker::resume
    pub fn suspend(&mut self, now: i64) -> Option<SessionReport> {
        let report = self.take_report(now);
        if let Phase::Active { uuid, .. } = &self.phase {
            let uuid = uuid.clone();
            self.phase = Phase::Paused { uuid };
        }
        report
    }

    /// Restart the segment for a book previously [`suspend`]ed. No-op when
    /// idle or already active.
    ///
    /// [`suspend`]: SessionTracker::suspend
    pub fn resume(&mut self, now: i64) {
        if let Phase::Paused { uuid } = &self.phase {
            let uuid = uuid.clone();
            self.phase = Phase::Active {
                uuid,
                started_at: now,
            };
        }
    }

    /// Heartbeat: emit the running segment as a report and restart it at
    /// `now`, so a killed process loses at most one heartbeat interval.
    /// Segments still under [`MIN_SESSION_SECS`] keep accruing unreported.
    pub fn rollover(&mut self, now: i64) -> Option<SessionReport> {
        let format = self.format;
        let Phase::Active { uuid, started_at } = &mut self.phase else {
            return None;
        };
        if now.saturating_sub(*started_at) < MIN_SESSION_SECS {
            return None;
        }
        let start = *started_at;
        *started_at = now;
        build_report(uuid, format, start, now)
    }

    /// Build the report for the current segment without changing phase.
    fn take_report(&self, now: i64) -> Option<SessionReport> {
        let Phase::Active { uuid, started_at } = &self.phase else {
            return None;
        };
        build_report(uuid, self.format, *started_at, now)
    }
}

/// One report spanning `[start, end]`, or `None` when the span is shorter
/// than [`MIN_SESSION_SECS`] (which also rejects a backwards clock).
fn build_report(uuid: &str, format: ProgressFormat, start: i64, end: i64) -> Option<SessionReport> {
    if end.saturating_sub(start) < MIN_SESSION_SECS {
        return None;
    }
    Some(SessionReport {
        book_uuid: uuid.to_string(),
        format,
        started_at: start,
        ended_at: end,
        // Active wall-clock seconds — the stats aggregates SUM this.
        progress_units: end - start,
        device_id: None,
        // Web posts a report once, on unmount, and never retries it — there
        // is no replay for a handle to make idempotent.
        client_id: None,
    })
}

/// Shared handle: the hooks mutate the tracker from effect closures, JS
/// event listeners, and drop handlers, all on the UI thread.
type SharedTracker = Rc<RefCell<SessionTracker>>;

/// Track a reading session for the book open in the reader: the segment
/// runs while the page is mounted (and, on web, the tab visible), with a
/// heartbeat flush every [`HEARTBEAT_SECS`]. Call unconditionally from the
/// reader page — every internal hook runs on all targets (rule 07); on SSR
/// the effects never fire and this is a no-op.
pub fn use_reading_session(uuid: String, server_url: String) {
    let tracker: SharedTracker =
        use_hook(|| Rc::new(RefCell::new(SessionTracker::new(ProgressFormat::Epub))));

    // Web: flush on tab-hide, resume on tab-show. `sendBeacon` delivers the
    // hidden-flush even when the tab is being closed. Installed post-mount;
    // the holder keeps the JS closure alive until removal in `use_drop`.
    #[cfg(feature = "web")]
    let vis_holder = use_hook(|| Rc::new(RefCell::new(None::<web::VisibilityFlush>)));
    #[cfg(feature = "web")]
    {
        let tracker = tracker.clone();
        let vis_holder = vis_holder.clone();
        use_effect(move || {
            *vis_holder.borrow_mut() = web::install_visibility_flush(tracker.clone());
        });
    }

    // Start on mount, switch (flushing the old segment) on SPA nav between
    // books — route param changes reuse the mounted page component.
    {
        let tracker = tracker.clone();
        let server_url_fx = server_url.clone();
        use_effect(use_reactive!(|uuid| {
            let report = tracker.borrow_mut().start(&uuid, now_secs());
            dispatch(&server_url_fx, report);
        }));
    }

    use_heartbeat(tracker.clone(), server_url.clone());

    // Final flush when the reader unmounts (SPA nav away).
    use_drop(move || {
        #[cfg(feature = "web")]
        web::remove_visibility_flush(vis_holder.borrow_mut().take());
        let report = tracker.borrow_mut().stop(now_secs());
        dispatch(&server_url, report);
    });
}

/// Track listening sessions off the app-root playback signals: the segment
/// runs while `playing` is true for a `uuid`, ends on pause/stop/book
/// swap, with the same heartbeat flush. Both hosts (web `PlaybackState`,
/// mobile `MobilePlayback`) expose exactly these two signals. `server_url`
/// is a signal because the hosts mount once at app root — on mobile the
/// URL is set by the Connect screen *after* mount, so it must be read at
/// dispatch time, not captured.
pub fn use_listening_session(
    uuid: Signal<Option<String>>,
    playing: Signal<bool>,
    server_url: Signal<String>,
) {
    let tracker: SharedTracker =
        use_hook(|| Rc::new(RefCell::new(SessionTracker::new(ProgressFormat::Audio))));

    // Web: a hidden tab keeps playing audio, so visibility must NOT end the
    // segment — only an actual page teardown (tab close / hard nav) does.
    // The holder keeps the JS closure alive for the app-root host's
    // lifetime (these hosts never unmount, so there is no removal path).
    #[cfg(feature = "web")]
    let hide_holder = use_hook(|| Rc::new(RefCell::new(None::<web::VisibilityFlush>)));
    #[cfg(feature = "web")]
    {
        let tracker = tracker.clone();
        let hide_holder = hide_holder.clone();
        use_effect(move || {
            *hide_holder.borrow_mut() = web::install_pagehide_flush(tracker.clone());
        });
    }

    {
        let tracker = tracker.clone();
        use_effect(move || {
            let report = match (uuid(), playing()) {
                (Some(u), true) => tracker.borrow_mut().start(&u, now_secs()),
                _ => tracker.borrow_mut().stop(now_secs()),
            };
            dispatch(&server_url.peek(), report);
        });
    }

    // Heartbeat, reading the live server URL each interval.
    use_future(move || {
        let tracker = tracker.clone();
        async move {
            #[cfg(any(feature = "web", feature = "mobile"))]
            loop {
                sleep_secs(HEARTBEAT_SECS).await;
                let report = tracker.borrow_mut().rollover(now_secs());
                dispatch(&server_url.peek(), report);
            }
            #[cfg(not(any(feature = "web", feature = "mobile")))]
            let _ = tracker;
        }
    });
}

/// Flush the running segment every [`HEARTBEAT_SECS`]. The task is tied to
/// the calling scope, so it dies with the reader page / lives forever on
/// the app-root player hosts. Completes immediately on SSR.
fn use_heartbeat(tracker: SharedTracker, server_url: String) {
    use_future(move || {
        let tracker = tracker.clone();
        let server_url = server_url.clone();
        async move {
            #[cfg(any(feature = "web", feature = "mobile"))]
            loop {
                sleep_secs(HEARTBEAT_SECS).await;
                let report = tracker.borrow_mut().rollover(now_secs());
                dispatch(&server_url, report);
            }
            #[cfg(not(any(feature = "web", feature = "mobile")))]
            let _ = (tracker, server_url);
        }
    });
}

/// Fire-and-forget a report to the session-ingest endpoint. Telemetry, not
/// user data — a lost batch costs at most one heartbeat of stats time, so
/// failures are deliberately not surfaced.
fn dispatch(server_url: &str, report: Option<SessionReport>) {
    let Some(report) = report else { return };
    #[cfg(feature = "web")]
    {
        let _ = server_url; // web posts same-origin
        web::send_beacon(&[report]);
    }
    #[cfg(all(feature = "mobile", not(feature = "web")))]
    {
        let server_url = server_url.to_string();
        // Root-scope spawn: the final flush runs from `use_drop`, where the
        // component's own scope is already being torn down.
        dioxus::core::spawn_forever(async move {
            let _ = crate::data::record_sessions(&server_url, vec![report]).await;
        });
    }
    #[cfg(not(any(feature = "web", feature = "mobile")))]
    let _ = (server_url, report);
}

/// Current unix time in seconds from the platform clock.
fn now_secs() -> i64 {
    #[cfg(feature = "web")]
    {
        (js_sys::Date::now() / 1000.0) as i64
    }
    #[cfg(all(feature = "mobile", not(feature = "web")))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
    #[cfg(not(any(feature = "web", feature = "mobile")))]
    {
        0
    }
}

/// Async sleep on whichever timer the platform has (gloo on WASM, tokio on
/// native mobile).
#[cfg(any(feature = "web", feature = "mobile"))]
async fn sleep_secs(secs: u64) {
    #[cfg(feature = "web")]
    gloo_timers::future::TimeoutFuture::new((secs * 1000) as u32).await;
    #[cfg(all(feature = "mobile", not(feature = "web")))]
    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
}

/// Web-only interop: `sendBeacon` delivery and the visibility/pagehide
/// listeners that flush a segment when the tab hides or closes.
#[cfg(feature = "web")]
mod web {
    use wasm_bindgen::prelude::*;

    use super::{now_secs, SharedTracker};

    /// Keeps a registered JS listener's `Closure` alive; dropping it after
    /// `remove_visibility_flush` detaches the callback cleanly.
    pub(super) struct VisibilityFlush {
        target: web_sys::EventTarget,
        event: &'static str,
        closure: Closure<dyn FnMut()>,
    }

    /// POST reports to the session RPC via `navigator.sendBeacon`, which
    /// the browser delivers even during page teardown. Body matches the
    /// server-function arg shape (`{"reports": [...]}`), mirroring the raw
    /// progress POST in `reader::interop`.
    pub(super) fn send_beacon(reports: &[omnibus_shared::SessionReport]) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(json) = serde_json::to_string(&serde_json::json!({ "reports": reports })) else {
            return;
        };
        let parts = js_sys::Array::of1(&JsValue::from_str(&json));
        let opts = web_sys::BlobPropertyBag::new();
        opts.set_type("application/json");
        let Ok(blob) = web_sys::Blob::new_with_str_sequence_and_options(&parts, &opts) else {
            return;
        };
        let _ = window
            .navigator()
            .send_beacon_with_opt_blob("/api/rpc/progress/sessions", Some(&blob));
    }

    /// Reader flush: suspend (and beacon) on tab-hide, resume on tab-show.
    pub(super) fn install_visibility_flush(tracker: SharedTracker) -> Option<VisibilityFlush> {
        let document = web_sys::window()?.document()?;
        let doc_for_cb = document.clone();
        let closure = Closure::new(move || {
            let mut t = tracker.borrow_mut();
            if doc_for_cb.visibility_state() == web_sys::VisibilityState::Hidden {
                let report = t.suspend(now_secs());
                drop(t);
                if let Some(r) = report {
                    send_beacon(&[r]);
                }
            } else {
                t.resume(now_secs());
            }
        });
        install(document.into(), "visibilitychange", closure)
    }

    /// Player flush: a hidden tab keeps playing, so only `pagehide` (tab
    /// close / hard navigation) ends the segment.
    pub(super) fn install_pagehide_flush(tracker: SharedTracker) -> Option<VisibilityFlush> {
        let window = web_sys::window()?;
        let closure = Closure::new(move || {
            let report = tracker.borrow_mut().stop(now_secs());
            if let Some(r) = report {
                send_beacon(&[r]);
            }
        });
        install(window.into(), "pagehide", closure)
    }

    fn install(
        target: web_sys::EventTarget,
        event: &'static str,
        closure: Closure<dyn FnMut()>,
    ) -> Option<VisibilityFlush> {
        target
            .add_event_listener_with_callback(event, closure.as_ref().unchecked_ref())
            .ok()?;
        Some(VisibilityFlush {
            target,
            event,
            closure,
        })
    }

    /// Detach a listener installed by the `install_*` helpers, so the page
    /// component's death doesn't leave JS calling a dropped closure.
    pub(super) fn remove_visibility_flush(flush: Option<VisibilityFlush>) {
        let Some(f) = flush else { return };
        let _ = f
            .target
            .remove_event_listener_with_callback(f.event, f.closure.as_ref().unchecked_ref());
    }
}
