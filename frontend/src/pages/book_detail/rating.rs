//! Interactive half-star rating widget for the book-detail hero card.
//!
//! Five stars with left/right half-click targets plus a status line. The first
//! paint matches SSR (no fill, "Not rated yet"); a post-mount effect loads the
//! saved rating and reconciles, keeping hydration stable. Clicking the active
//! value clears it (un-rate).

use dioxus::prelude::*;
use omnibus_shared::{AttributedRating, RatingRecord, RatingUpdate};

use crate::{data, use_server_url};

/// Live rating signals shared between [`BdRatingWidget`] and each
/// [`BdStarHalf`] click target: the persisted record, hover preview, failed
/// flag, and the monotonic op counter. Grouped to keep the half under the cap.
#[derive(Clone, Copy, PartialEq)]
struct RatingState {
    current: Signal<Option<RatingRecord>>,
    hover: Signal<Option<f32>>,
    failed: Signal<bool>,
    op_seq: Signal<u64>,
    other_ratings: Signal<Vec<AttributedRating>>,
}

/// Clickable 0.5–5.0 star rating bound to the current user and `uuid`.
#[component]
pub(super) fn BdRatingWidget(uuid: String) -> Element {
    let server_url = use_server_url();
    // `current` is the persisted rating; `hover` previews a pending pick. Both
    // seed to `None` so SSR and first-hydration paint render an unrated card —
    // the effect below reconciles on the client only.
    let mut current = use_signal(|| None::<RatingRecord>);
    let mut hover = use_signal(|| None::<f32>);
    let mut failed = use_signal(|| false);
    // Monotonic write counter: a save only applies its result if it's still the
    // latest, so an out-of-order (slow) response can't clobber a newer click.
    let mut op_seq = use_signal(|| 0u64);
    let mut load_seq = use_signal(|| 0u64);
    let mut other_ratings = use_signal(Vec::<AttributedRating>::new);
    let state = RatingState {
        current,
        hover,
        failed,
        op_seq,
        other_ratings,
    };

    let load_url = server_url.clone();
    use_effect(use_reactive!(|uuid| {
        if uuid.is_empty() {
            return;
        }
        let my_load = *load_seq.peek() + 1;
        let next_op = *op_seq.peek() + 1;
        load_seq.set(my_load);
        op_seq.set(next_op);
        current.set(None);
        other_ratings.set(Vec::new());
        failed.set(false);
        let load_url = load_url.clone();
        let uuid = uuid.clone();
        spawn(async move {
            if let Ok(rec) = data::get_rating(&load_url, &uuid).await {
                if *load_seq.peek() == my_load {
                    current.set(rec);
                }
            }
            if let Ok(ratings) = data::list_other_ratings(&load_url, &uuid).await {
                if *load_seq.peek() == my_load {
                    other_ratings.set(ratings);
                }
            }
        });
    }));

    // The fill the user sees: a hover preview wins over the saved value.
    let shown = hover()
        .or_else(|| current().map(|r| r.stars))
        .unwrap_or(0.0);

    let meta_text = if failed() {
        "Couldn't save rating — try again".to_string()
    } else if let Some(rec) = current() {
        format!(
            "{} \u{00b7} {} of 5",
            rated_ago(rec.updated_at),
            fmt_stars(rec.stars)
        )
    } else {
        "Not rated yet".to_string()
    };

    rsx! {
        div {
            class: "bd-stars bd-stars-interactive",
            role: "group",
            aria_label: "Your rating",
            "data-testid": "rating-stars",
            onmouseleave: move |_| hover.set(None),
            for i in 1..=5u8 {
                {
                    let slot = i as f32;
                    let fill = if shown >= slot {
                        100
                    } else if shown >= slot - 0.5 {
                        50
                    } else {
                        0
                    };
                    rsx! {
                        span { key: "{i}", class: "bd-star-slot",
                            span { class: "bd-star-bg", "\u{2605}" }
                            span { class: "bd-star-fg", style: "width: {fill}%", "\u{2605}" }
                            BdStarHalf {
                                value: slot - 0.5,
                                side: "left",
                                uuid: uuid.clone(),
                                server_url: server_url.clone(),
                                state,
                            }
                            BdStarHalf {
                                value: slot,
                                side: "right",
                                uuid: uuid.clone(),
                                server_url: server_url.clone(),
                                state,
                            }
                        }
                    }
                }
            }
        }
        div {
            class: "mono bd-rating-meta",
            role: "status",
            "data-testid": "rating-meta",
            "{meta_text}"
        }
        if !other_ratings().is_empty() {
            div {
                class: "bd-other-ratings",
                "data-testid": "other-ratings",
                aria_label: "Other ratings",
                for rating in other_ratings() {
                    BdOtherRatingRow {
                        key: "{rating.user_id}",
                        rating,
                    }
                }
            }
        }
    }
}

/// One half-star click target. `value` is the rating it selects (`x.5` for the
/// left half, `x` for the right); re-clicking the active value clears it.
#[component]
fn BdStarHalf(
    value: f32,
    side: &'static str,
    uuid: String,
    server_url: String,
    state: RatingState,
) -> Element {
    let RatingState {
        current,
        mut hover,
        failed,
        op_seq,
        other_ratings,
    } = state;
    rsx! {
        button {
            r#type: "button",
            class: "bd-star-half bd-star-half-{side}",
            aria_label: rate_label(value),
            onmouseenter: move |_| hover.set(Some(value)),
            onclick: move |_| {
                if uuid.is_empty() {
                    return;
                }
                // Re-clicking the active value clears the rating (un-rate).
                let target = if current().map(|r| r.stars) == Some(value) {
                    None
                } else {
                    Some(value)
                };
                apply_rating(
                    current,
                    failed,
                    op_seq,
                    other_ratings,
                    uuid.clone(),
                    server_url.clone(),
                    target,
                );
            },
        }
    }
}

/// Optimistically set/clear `current`, then persist and reconcile with the
/// server (reverting and flagging on error). `Signal` is `Copy`, so the handles
/// are passed by value. `op_seq` is bumped per call and re-checked after the
/// request so a stale (out-of-order) response is dropped instead of clobbering a
/// newer click.
fn apply_rating(
    mut current: Signal<Option<RatingRecord>>,
    mut failed: Signal<bool>,
    mut op_seq: Signal<u64>,
    mut other_ratings: Signal<Vec<AttributedRating>>,
    uuid: String,
    server_url: String,
    target: Option<f32>,
) {
    let prev = current();
    let my_op = op_seq() + 1;
    op_seq.set(my_op);
    failed.set(false);
    current.set(target.map(|stars| RatingRecord {
        book_uuid: uuid.clone(),
        stars,
        updated_at: now_unix(),
    }));
    spawn(async move {
        let result = match target {
            Some(stars) => data::set_rating(
                &server_url,
                RatingUpdate {
                    book_uuid: uuid.clone(),
                    stars,
                },
            )
            .await
            .map(Some),
            None => data::clear_rating(&server_url, &uuid).await.map(|()| None),
        };
        // A newer click superseded this one — drop the stale result.
        if op_seq() != my_op {
            return;
        }
        match result {
            Ok(rec) => {
                current.set(rec);
                if let Ok(ratings) = data::list_other_ratings(&server_url, &uuid).await {
                    other_ratings.set(ratings);
                }
            }
            Err(_) => {
                current.set(prev);
                failed.set(true);
            }
        }
    });
}

#[component]
fn BdOtherRatingRow(rating: AttributedRating) -> Element {
    let initial = rating
        .username
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string());
    let age = rating_age_days(now_unix(), rating.updated_at);
    rsx! {
        div {
            class: "bd-other-rating-row",
            "data-testid": "other-rating-row",
            span {
                class: "bd-other-rating-avatar",
                aria_hidden: "true",
                "{initial}"
            }
            span { class: "bd-other-rating-name", "{rating.username}" }
            span { "rated" }
            BdOtherRatingStars { stars: rating.stars }
            span { class: "mono bd-other-rating-age", "{age}" }
        }
    }
}

#[component]
fn BdOtherRatingStars(stars: f32) -> Element {
    rsx! {
        span {
            class: "bd-other-rating-stars",
            aria_label: "{fmt_stars(stars)} out of 5 stars",
            for i in 1..=5u8 {
                {
                    let slot = i as f32;
                    let fill = if stars >= slot {
                        100
                    } else if stars >= slot - 0.5 {
                        50
                    } else {
                        0
                    };
                    rsx! {
                        span {
                            key: "{i}",
                            class: "bd-other-rating-star-slot",
                            aria_hidden: "true",
                            span { class: "bd-star-bg", "\u{2605}" }
                            span {
                                class: "bd-star-fg",
                                style: "width: {fill}%",
                                "\u{2605}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Render a star value without a trailing `.0` (`4.5` stays, `4.0` → `4`).
fn fmt_stars(v: f32) -> String {
    if v.fract().abs() < f32::EPSILON {
        format!("{}", v as i64)
    } else {
        format!("{v:.1}")
    }
}

/// Accessible label for a half-star button.
fn rate_label(value: f32) -> String {
    if (value - 1.0).abs() < f32::EPSILON {
        "Rate 1 star".to_string()
    } else {
        format!("Rate {} stars", fmt_stars(value))
    }
}

/// Relative "rated N ago" phrase from a unix-seconds timestamp.
fn rated_ago(updated_at: i64) -> String {
    let secs = (now_unix() - updated_at).max(0);
    let plural = |n: i64| if n == 1 { "" } else { "s" };
    if secs < 60 {
        "rated just now".to_string()
    } else if secs < 3600 {
        let m = secs / 60;
        format!("rated {m} minute{} ago", plural(m))
    } else if secs < 86_400 {
        let h = secs / 3600;
        format!("rated {h} hour{} ago", plural(h))
    } else {
        let d = secs / 86_400;
        format!("rated {d} day{} ago", plural(d))
    }
}

fn rating_age_days(now: i64, updated_at: i64) -> String {
    let days = (now - updated_at).max(0) / 86_400;
    match days {
        0 => "today".to_string(),
        1 => "1 day ago".to_string(),
        _ => format!("{days} days ago"),
    }
}

/// Current unix time in seconds. Only ever called client-side (the relative
/// timestamp renders after the post-mount load, and the optimistic write runs
/// on click), so SSR never invokes the JS clock.
fn now_unix() -> i64 {
    #[cfg(feature = "web")]
    {
        (js_sys::Date::now() / 1000.0) as i64
    }
    #[cfg(not(feature = "web"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::rating_age_days;

    #[test]
    fn rating_age_uses_today_for_recent_rating() {
        assert_eq!(rating_age_days(1_000, 999), "today");
    }

    #[test]
    fn rating_age_singularizes_one_day() {
        assert_eq!(rating_age_days(172_800, 86_400), "1 day ago");
    }

    #[test]
    fn rating_age_pluralizes_multiple_days() {
        assert_eq!(rating_age_days(432_000, 172_800), "3 days ago");
    }
}
