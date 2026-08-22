//! `POST v1/analytics/event` — the device's telemetry channel, mined for the
//! two event types that map onto real user data: `LeaveContent` (a reading
//! session) and `RateBook` (a star rating). Everything else is acknowledged
//! and dropped.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db as db;
use omnibus_shared::{ProgressFormat, RatingUpdate, SessionReport};
use serde::Deserialize;

use super::extractor::KoboAuthUser;
use super::AppState;

/// Maximum number of `events` entries accepted per analytics-batch POST,
/// mirroring [`super::dto::StateRequest::MAX_READING_STATES`] and
/// [`omnibus_shared::SESSION_BATCH_CAP`] — a real device batches a handful of
/// events per sync, so this is generous headroom, not a realistic ceiling.
pub(super) const MAX_ANALYTICS_EVENTS: usize = 256;

/// Cap an analytics batch at [`MAX_ANALYTICS_EVENTS`] entries before any DB
/// work, mirroring [`super::reject_oversized_uuid`]. `Some(response)` is the
/// rejection.
fn reject_oversized_analytics_batch(body: &AnalyticsBody) -> Option<Response> {
    (body.events.len() > MAX_ANALYTICS_EVENTS).then(|| StatusCode::BAD_REQUEST.into_response())
}

/// The analytics batch body. Unknown fields (`PlatformId`, `SerialNumber`, …)
/// are ignored by serde.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AnalyticsBody {
    #[serde(default)]
    pub events: Vec<AnalyticsEvent>,
}

/// One device event. Only `LeaveContent` and `RateBook` are consumed; the
/// field grab-bags are typed loosely because the device's shapes vary by
/// firmware and an analytics decode failure must never fail the request.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AnalyticsEvent {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub event_type: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub metrics: Metrics,
    #[serde(default)]
    pub attributes: Attributes,
}

/// The measured half of an analytics event: seconds read, and a star rating
/// when the device sends one.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Metrics {
    #[serde(default)]
    pub seconds_read: Option<f64>,
    /// Lowercase on the wire, unlike the PascalCase siblings.
    #[serde(default, rename = "stars")]
    pub stars: Option<f64>,
}

/// The subject half of an analytics event — which book it is about.
#[derive(Debug, Default, Deserialize)]
pub struct Attributes {
    /// The book uuid, in the device's lowercase key.
    #[serde(default, rename = "volumeid")]
    pub volumeid: String,
}

/// Ingest a batch. Answers `{"Result": "Success"}` for any batch within
/// `MAX_ANALYTICS_EVENTS` — analytics is best-effort by contract, and a 4xx/5xx
/// here makes the device re-queue and hammer the route, so per-event failures
/// are logged and skipped rather than surfaced. An oversized batch is the one
/// exception, rejected with 400 before any DB work.
pub async fn analytics_event(
    auth: KoboAuthUser,
    State(state): State<AppState>,
    Json(body): Json<AnalyticsBody>,
) -> Response {
    if let Some(rejected) = reject_oversized_analytics_batch(&body) {
        return rejected;
    }
    for event in &body.events {
        let outcome = match event.event_type.as_str() {
            "LeaveContent" => ingest_session(&state, auth.user_id, event).await,
            "RateBook" => ingest_rating(&state, auth.user_id, event).await,
            _ => Ok(()),
        };
        if let Err(msg) = outcome {
            tracing::warn!(
                user_id = auth.user_id,
                event_id = %event.id,
                event_type = %event.event_type,
                error = %msg,
                "kobo analytics event skipped"
            );
        }
    }
    Json(serde_json::json!({ "Result": "Success" })).into_response()
}

/// `GET`/`POST v1/analytics/gettests` — the A/B-test flag fetch the
/// initialization map points here (real firmware POSTs it). No tests to run;
/// an empty result is a valid answer.
pub async fn analytics_gettests(_auth: KoboAuthUser) -> Response {
    Json(serde_json::json!({ "Result": "Success", "TestKey": "", "Tests": {} })).into_response()
}

/// `LeaveContent` → one `reading_sessions` row. The event `Id` rides as the
/// session `client_id`, so the device re-posting a batch it never got an ack
/// for collapses onto the row it already wrote (migration 0052).
async fn ingest_session(
    state: &AppState,
    user_id: i64,
    event: &AnalyticsEvent,
) -> Result<(), String> {
    // The finite/positive guard plus the clamp keep the cast in-range.
    #[allow(clippy::cast_possible_truncation)]
    let seconds = match event.metrics.seconds_read {
        Some(s) if s.is_finite() && s >= 1.0 => s.min(f64::from(i32::MAX)) as i64,
        // A zero/absent duration is a glance, not a session — drop silently.
        _ => return Ok(()),
    };
    if event.attributes.volumeid.is_empty() {
        return Err("LeaveContent without a volumeid".into());
    }
    let ended_at = parse_epoch(&event.timestamp)
        .unwrap_or_else(|| time::OffsetDateTime::now_utc().unix_timestamp());
    let report = SessionReport {
        book_uuid: event.attributes.volumeid.clone(),
        format: ProgressFormat::Epub,
        started_at: ended_at - seconds,
        ended_at,
        progress_units: seconds,
        // `devices.id` is the mobile-device table; a Kobo has no row there.
        device_id: None,
        client_id: (!event.id.is_empty()).then(|| format!("kobo:{}", event.id)),
    };
    // Same validator the web/mobile session-report path runs — rejects a
    // pre-epoch device clock (`started_at < 0`) or an oversized `SecondsRead`
    // before the row reaches `reading_sessions`, where it would skew
    // aggregates indefinitely (see the validator's own doc comment).
    report.validate()?;
    match db::progress::record_session(state.pool(), user_id, &report).await {
        Ok(_recorded) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// `RateBook` → `user_ratings`. Kobo sends whole stars 0–5; 0 clears.
async fn ingest_rating(
    state: &AppState,
    user_id: i64,
    event: &AnalyticsEvent,
) -> Result<(), String> {
    if event.attributes.volumeid.is_empty() {
        return Err("RateBook without a volumeid".into());
    }
    let stars = match event.metrics.stars {
        Some(s) if s.is_finite() => s,
        _ => return Err("RateBook without a stars metric".into()),
    };
    if stars == 0.0 {
        return db::ratings::delete_rating(state.pool(), user_id, &event.attributes.volumeid)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string());
    }
    // Whole stars 1..=5 survive f64 → f32 exactly; anything out of range is
    // rejected by the validator right below.
    #[allow(clippy::cast_possible_truncation)]
    let update = RatingUpdate {
        book_uuid: event.attributes.volumeid.clone(),
        stars: stars as f32,
    };
    // Reuse the shared validator so the Kobo path rejects exactly what the
    // web/mobile rating paths reject (range, half-star steps).
    update.validate()?;
    db::ratings::set_rating(state.pool(), user_id, &update)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Parse the device's RFC 3339 timestamp to epoch seconds; `None` on any
/// deviation — the caller falls back to server time.
fn parse_epoch(ts: &str) -> Option<i64> {
    time::OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|t| t.unix_timestamp())
}
