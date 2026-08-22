//! Remembering that a provider rate-limited us, so the next search doesn't
//! walk straight back into the same 429.
//!
//! A per-request failure status is not enough: it tells the reader this
//! search failed, then the next click asks the same exhausted API again, and
//! the one after that. Google Books in particular answers a keyless instance
//! from a shared anonymous daily quota, so once it starts refusing it will
//! keep refusing for hours — and every one of those requests costs the reader
//! a spinner and the provider a request it has already said no to.
//!
//! State is in memory and per process. A restart clears every cooldown, which
//! is the right default: the cooldown is a courtesy, not a ledger, and an
//! operator restarting the server to fix something should get a clean try.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use omnibus_shared::metadata_lookup::MetadataProvider;

/// How long to wait after each successive 429, in seconds.
///
/// Escalates only while a cooldown is *still running* — a 429 that arrives
/// after the previous one lapsed starts again at the first step, since the
/// provider evidently let us back in.
const DEFAULT_SCHEDULE: &[u64] = &[120, 300, 600, 1800, 3600];

/// Google Books' free tier is a daily quota, not a per-minute rate: once it is
/// spent, backing off for two minutes just spends another request finding out
/// it is still spent. Its steps are correspondingly longer.
const GOOGLE_BOOKS_SCHEDULE: &[u64] = &[600, 1800, 3600, 7200, 14400];

/// The longest a provider is ever parked for.
const MAX_COOLDOWN: Duration = Duration::from_secs(24 * 60 * 60);

/// The schedule for one provider.
fn schedule_for(provider: MetadataProvider) -> &'static [u64] {
    match provider {
        MetadataProvider::GoogleBooks => GOOGLE_BOOKS_SCHEDULE,
        MetadataProvider::OpenLibrary | MetadataProvider::Hardcover => DEFAULT_SCHEDULE,
    }
}

/// One provider's cooldown.
#[derive(Debug, Clone, Copy)]
struct Cooldown {
    until: Instant,
    /// How many 429s have landed without a successful call in between. Indexes
    /// [`schedule_for`], saturating at its last step.
    level: usize,
}

/// A shared record of which providers are cooling down.
///
/// Cloning shares the state — it is a handle, not a copy — which is what lets
/// it ride along on [`super::MetadataLookupConfig`] and reach both the REST
/// handler and the web server function. Those two build their configs
/// independently and neither can see the other's `AppState`, so a tracker
/// hung off the router would silently exempt half the requests.
#[derive(Debug, Clone)]
pub struct ThrottleTracker(Arc<Mutex<HashMap<MetadataProvider, Cooldown>>>);

impl ThrottleTracker {
    /// The process-wide tracker. What production uses, so a cooldown learned
    /// while serving one request is known to the next.
    pub fn shared() -> Self {
        static SHARED: OnceLock<ThrottleTracker> = OnceLock::new();
        SHARED
            .get_or_init(|| ThrottleTracker(Arc::new(Mutex::new(HashMap::new()))))
            .clone()
    }

    /// An isolated tracker. For tests, which must not inherit — or leave
    /// behind — a cooldown from another test.
    pub fn fresh() -> Self {
        ThrottleTracker(Arc::new(Mutex::new(HashMap::new())))
    }

    /// How long is left on this provider's cooldown, or `None` if it may be
    /// asked. Expired entries are dropped as they are found.
    pub fn remaining(&self, provider: MetadataProvider) -> Option<Duration> {
        self.remaining_at(provider, Instant::now())
    }

    /// Note that `provider` answered 429.
    ///
    /// `retry_after` is the provider's own `Retry-After` when it sent one; it
    /// overrides the schedule's duration but still advances the level, so a
    /// provider that keeps saying "one more minute" is eventually taken at
    /// more than its word.
    pub fn record(&self, provider: MetadataProvider, retry_after: Option<Duration>) {
        self.record_at(provider, retry_after, Instant::now());
    }

    /// Note that `provider` answered normally, clearing any cooldown and
    /// resetting its escalation.
    ///
    /// This is the only thing that resets the level, which is why it must be
    /// called on evidence the provider actually served a request — not merely
    /// because a call returned `Ok`, since a provider client can swallow its
    /// own failure (`googlebooks::by_isbn` degrades a refused fallback to a
    /// clean miss) and answer `Ok` with a cooldown freshly recorded.
    pub fn clear(&self, provider: MetadataProvider) {
        self.with(|map| {
            map.remove(&provider);
        });
    }

    /// [`Self::remaining`] against a caller-supplied clock, so the escalation
    /// and expiry rules are testable without sleeping.
    pub fn remaining_at(&self, provider: MetadataProvider, now: Instant) -> Option<Duration> {
        self.with(|map| {
            let cooldown = *map.get(&provider)?;
            (cooldown.until > now).then(|| cooldown.until - now)
        })
    }

    /// [`Self::record`] against a caller-supplied clock.
    pub fn record_at(
        &self,
        provider: MetadataProvider,
        retry_after: Option<Duration>,
        now: Instant,
    ) {
        self.with(|map| {
            // Escalates from the level already recorded, whether or not that
            // cooldown has lapsed. Pruning the level on lapse made every
            // schedule one step long: the gates mean a provider is never asked
            // *during* a cooldown, so a second 429 can only ever arrive after
            // one expired — and a level reset at that moment could never
            // advance. The level is cleared by [`Self::clear`], on evidence
            // that the provider is answering again.
            let previous = map.get(&provider).copied();
            let level = previous.map_or(0, |c| c.level + 1);
            let schedule = schedule_for(provider);
            let step = schedule[level.min(schedule.len() - 1)];
            // Capped: `Retry-After` is a provider-supplied number, and
            // `Instant + Duration` *panics* on overflow — a header of
            // `999999999` would take the process down rather than merely
            // parking the provider for thirty-one years. Beyond a day a
            // cooldown is indistinguishable from "disabled" anyway, and a
            // restart clears it.
            // `Retry-After` is a floor, not a replacement. Taken verbatim it
            // made the escalation inert whenever a provider sent one, and a
            // provider that answers "try again in 1s" on its fifth refusal
            // would reset an hour-long cooldown to a second.
            let wait = retry_after
                .unwrap_or_default()
                .max(Duration::from_secs(step))
                .min(MAX_COOLDOWN);
            // Never shortens a cooldown that is still running.
            let until = previous
                .map(|c| c.until.max(now + wait))
                .unwrap_or(now + wait);
            map.insert(provider, Cooldown { until, level });
        });
    }

    /// Run `f` against the map, recovering a poisoned lock rather than
    /// propagating a panic — a throttle record is advisory, and losing the
    /// whole feature because one thread panicked mid-update helps nobody.
    fn with<T>(&self, f: impl FnOnce(&mut HashMap<MetadataProvider, Cooldown>) -> T) -> T {
        let mut guard = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut guard)
    }
}

impl Default for ThrottleTracker {
    /// The shared tracker — production's answer, so a config built without
    /// thinking about throttling still participates in it.
    fn default() -> Self {
        Self::shared()
    }
}

/// Whole seconds left, rounded up, for the wire.
pub fn retry_after_secs(remaining: Duration) -> u64 {
    remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0)
}

/// Parse a `Retry-After` header: either delta-seconds or an HTTP-date.
///
/// A value that is absent, unparseable, zero, or already in the past yields
/// `None`, which sends the caller back to the schedule — a provider naming a
/// moment that has passed has told us nothing.
pub fn parse_retry_after(value: Option<&str>) -> Option<Duration> {
    let raw = value?.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(secs) = raw.parse::<u64>() {
        return (secs > 0).then(|| Duration::from_secs(secs));
    }
    let at = httpdate::parse_http_date(raw).ok()?;
    at.duration_since(std::time::SystemTime::now()).ok()
}

#[cfg(test)]
mod tests;
