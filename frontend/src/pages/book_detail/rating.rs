//! Interactive half-star rating widget for the book-detail hero card.
//!
//! Five stars with left/right half-click targets plus a status line. The first
//! paint matches SSR (no fill, "Not rated yet"); a post-mount effect loads the
//! saved rating and reconciles, keeping hydration stable. Clicking the active
//! value clears it (un-rate).

use dioxus::prelude::*;
use omnibus_shared::external_ratings::ExternalRating;
use omnibus_shared::{AttributedRating, RatingRecord, RatingUpdate};

use crate::components::user_avatar::UserAvatar;
use crate::time::now_unix;
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
    community: Signal<Vec<ExternalRating>>,
}

/// Clickable 0.5–5.0 star rating bound to the current user and `uuid`.
#[component]
pub(super) fn BdRatingWidget(uuid: String) -> Element {
    let server_url = use_server_url();
    // `current` is the persisted rating; `hover` previews a pending pick. Both
    // seed to `None` so SSR and first-hydration paint render an unrated card —
    // the effect below reconciles on the client only.
    let current = use_signal(|| None::<RatingRecord>);
    let mut hover = use_signal(|| None::<f32>);
    let failed = use_signal(|| false);
    // Monotonic write counter: a save only applies its result if it's still the
    // latest, so an out-of-order (slow) response can't clobber a newer click.
    let op_seq = use_signal(|| 0u64);
    let other_ratings = use_signal(Vec::<AttributedRating>::new);
    let community = use_signal(Vec::<ExternalRating>::new);
    let state = RatingState {
        current,
        hover,
        failed,
        op_seq,
        other_ratings,
        community,
    };

    use_rating_hydration(uuid.clone(), server_url.clone(), state);

    // The fill the user sees: a hover preview wins over the saved value.
    let shown = hover()
        .or_else(|| current().map(|r| r.stars))
        .unwrap_or(0.0);
    let meta_text = rating_meta_text(failed(), current());

    rsx! {
        div {
            class: "bd-stars bd-stars-interactive",
            role: "group",
            aria_label: "Your rating",
            "data-testid": "rating-stars",
            onmouseleave: move |_| hover.set(None),
            for i in 1..=5u8 {
                BdStarSlot {
                    key: "{i}",
                    slot: i as f32,
                    shown,
                    uuid: uuid.clone(),
                    server_url: server_url.clone(),
                    state,
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
            BdOtherRatingsList { ratings: other_ratings() }
        }
        if !community().is_empty() {
            BdCommunityRatings { ratings: community() }
        }
    }
}

/// Loads the persisted rating and other-users' ratings for `uuid` on mount and
/// whenever `uuid` changes, guarding stale (out-of-order) responses with a
/// load-sequence counter. Called unconditionally from [`BdRatingWidget`].
fn use_rating_hydration(uuid: String, load_url: String, mut state: RatingState) {
    let mut load_seq = use_signal(|| 0u64);
    use_effect(use_reactive!(|uuid| {
        if uuid.is_empty() {
            return;
        }
        let my_load = *load_seq.peek() + 1;
        let next_op = *state.op_seq.peek() + 1;
        load_seq.set(my_load);
        state.op_seq.set(next_op);
        state.current.set(None);
        state.other_ratings.set(Vec::new());
        state.community.set(Vec::new());
        state.failed.set(false);
        let load_url = load_url.clone();
        let uuid = uuid.clone();
        let mut state = state;
        spawn(async move {
            if let Ok(rec) = data::get_rating(&load_url, &uuid).await {
                if *load_seq.peek() == my_load {
                    state.current.set(rec);
                }
            }
            if let Ok(ratings) = data::list_other_ratings(&load_url, &uuid).await {
                if *load_seq.peek() == my_load {
                    state.other_ratings.set(ratings);
                }
            }
            if let Ok(ratings) = data::list_external_ratings(&load_url, &uuid).await {
                if *load_seq.peek() == my_load {
                    state.community.set(ratings);
                }
            }
        });
    }));
}

/// The status line under the star row: an error takes priority, then the
/// saved rating's age and value, else the unrated placeholder.
fn rating_meta_text(failed: bool, current: Option<RatingRecord>) -> String {
    if failed {
        "Couldn't save rating — try again".to_string()
    } else if let Some(rec) = current {
        format!(
            "{} \u{00b7} {} of 5",
            rated_ago(now_unix(), rec.updated_at),
            fmt_stars(rec.stars)
        )
    } else {
        "Not rated yet".to_string()
    }
}

/// One star's slot in the row: background glyph, fill overlay sized to the
/// shown value, and its two half-star click targets.
#[component]
fn BdStarSlot(
    slot: f32,
    shown: f32,
    uuid: String,
    server_url: String,
    state: RatingState,
) -> Element {
    let fill = if shown >= slot {
        100
    } else if shown >= slot - 0.5 {
        50
    } else {
        0
    };
    rsx! {
        span { class: "bd-star-slot",
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

/// The "other readers' ratings" list shown under the interactive widget.
#[component]
fn BdOtherRatingsList(ratings: Vec<AttributedRating>) -> Element {
    rsx! {
        div {
            class: "bd-other-ratings",
            "data-testid": "other-ratings",
            aria_label: "Other ratings",
            for rating in ratings {
                BdOtherRatingRow {
                    key: "{rating.user_id}",
                    rating,
                }
            }
        }
    }
}

/// The community ratings the metadata providers publish, one row per source.
///
/// Shown **beside** the reader's own stars, never merged into them: these are
/// what the internet gave the book, on each source's own scale, and averaging
/// three strangers' scales into one number would answer neither question.
#[component]
fn BdCommunityRatings(ratings: Vec<ExternalRating>) -> Element {
    rsx! {
        div {
            class: "bd-community-ratings",
            "data-testid": "community-ratings",
            div { class: "label", "Community ratings" }
            for rating in ratings {
                BdCommunityRatingRow { key: "{rating.provider.as_str()}", rating }
            }
        }
    }
}

/// One source's score: `4.2/5 (1,840) · Google Books`, linking out to the
/// provider's own page when it gave one.
#[component]
fn BdCommunityRatingRow(rating: ExternalRating) -> Element {
    let score = format!(
        "{}/{}",
        fmt_score(rating.rating),
        fmt_score(rating.rating_max)
    );
    // Absent is not zero, so a source that reported no count shows none —
    // rather than "(0)", which would read as nobody having rated it.
    let count = rating
        .ratings_count
        .map(|n| format!(" ({})", fmt_count(n)))
        .unwrap_or_default();
    rsx! {
        div {
            class: "bd-community-rating-row mono",
            "data-testid": "community-rating-row",
            span { class: "bd-community-rating-score", "{score}{count}" }
            span { class: "bd-community-rating-sep", " · " }
            if let Some(url) = rating.source_url.clone() {
                a {
                    class: "bd-community-rating-source",
                    href: "{url}",
                    target: "_blank",
                    rel: "noopener noreferrer",
                    "{rating.display_name}"
                }
            } else {
                span { class: "bd-community-rating-source", "{rating.display_name}" }
            }
        }
    }
}

/// One decimal, dropping a trailing `.0` so `5.0` renders as `5`.
fn fmt_score(v: f64) -> String {
    let rounded = (v * 10.0).round() / 10.0;
    if (rounded.fract()).abs() < 0.05 {
        format!("{rounded:.0}")
    } else {
        format!("{rounded:.1}")
    }
}

/// Thousands-separated rating count (`1840` → `1,840`).
fn fmt_count(n: i64) -> String {
    let digits = n.abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
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
    let mut hover = state.hover;
    let current = state.current;
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
                apply_rating(state, uuid.clone(), server_url.clone(), target);
            },
        }
    }
}

/// Optimistically set/clear `state.current`, then persist and reconcile with
/// the server (reverting and flagging on error). Takes the whole
/// [`RatingState`] bundle rather than its sibling signals as separate
/// positional params — `Signal` is `Copy`, so the bundle is passed by value.
/// `op_seq` is bumped per call and re-checked after the request so a stale
/// (out-of-order) response is dropped instead of clobbering a newer click.
fn apply_rating(state: RatingState, uuid: String, server_url: String, target: Option<f32>) {
    let RatingState {
        mut current,
        mut failed,
        mut op_seq,
        mut other_ratings,
        ..
    } = state;
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
                    if op_seq() == my_op {
                        other_ratings.set(ratings);
                    }
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
    let age = rated_ago(now_unix(), rating.updated_at);
    rsx! {
        div {
            class: "bd-other-rating-row",
            "data-testid": "other-rating-row",
            UserAvatar {
                user_id: rating.user_id,
                name: rating.username.clone(),
                has_avatar: rating.has_avatar,
                class: "bd-other-rating-avatar",
            }
            div {
                class: "bd-other-rating-content",
                "data-testid": "other-rating-content",
                div {
                    class: "bd-other-rating-byline",
                    "data-testid": "other-rating-byline",
                    span { class: "bd-other-rating-name", "{rating.username}" }
                    " {age}"
                }
                BdOtherRatingStars { stars: rating.stars }
            }
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
        format!("{v:.0}")
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

/// Relative "rated N ago" phrase from a unix-seconds timestamp, computed
/// against the injected `now` (rather than reading the clock internally) so
/// every call site — the live widget and the other-ratings row alike — shares
/// one clock-injectable, unit-testable formatter.
fn rated_ago(now: i64, updated_at: i64) -> String {
    let secs = (now - updated_at).max(0);
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

#[cfg(test)]
mod tests {
    use super::{fmt_count, fmt_score, rated_ago};

    #[test]
    fn fmt_score_drops_a_trailing_zero_decimal() {
        assert_eq!(fmt_score(5.0), "5");
        assert_eq!(fmt_score(4.0), "4");
    }

    #[test]
    fn fmt_score_keeps_one_decimal_and_rounds_to_it() {
        assert_eq!(fmt_score(4.2), "4.2");
        assert_eq!(fmt_score(3.75), "3.8");
    }

    #[test]
    fn fmt_count_groups_thousands() {
        assert_eq!(fmt_count(1_840), "1,840");
        assert_eq!(fmt_count(999), "999");
        assert_eq!(fmt_count(1_000_000), "1,000,000");
    }

    #[test]
    fn rated_ago_shows_just_now_for_sub_minute_gap() {
        assert_eq!(rated_ago(1_000, 999), "rated just now");
    }

    #[test]
    fn rated_ago_singularizes_one_minute() {
        assert_eq!(rated_ago(1_060, 1_000), "rated 1 minute ago");
    }

    #[test]
    fn rated_ago_pluralizes_multiple_hours() {
        assert_eq!(rated_ago(1_000 + 3 * 3600, 1_000), "rated 3 hours ago");
    }

    #[test]
    fn rated_ago_singularizes_one_day() {
        assert_eq!(rated_ago(172_800, 86_400), "rated 1 day ago");
    }

    #[test]
    fn rated_ago_pluralizes_multiple_days() {
        assert_eq!(rated_ago(432_000, 172_800), "rated 3 days ago");
    }
}

// SSR render-smoke coverage of the widget body — separate module because these
// need the `server` feature (`dioxus::ssr`), while the pure `rated_ago` tests
// above run under any target.
#[cfg(all(test, feature = "server"))]
mod render_tests {
    use omnibus_shared::metadata_lookup::MetadataProvider;

    use super::*;
    use crate::test_support::render;

    #[test]
    fn rating_widget_first_paint_is_unrated_with_five_interactive_stars() {
        let html = render(rsx! {
            BdRatingWidget { uuid: "book-uuid".to_string() }
        });

        assert!(html.contains("data-testid=\"rating-stars\""));
        assert!(html.contains("bd-stars bd-stars-interactive"));
        assert!(html.contains("aria-label=\"Your rating\""));
        // The post-mount load doesn't run under SSR, so the card renders its
        // unrated first paint — matching what the client hydrates against.
        assert!(html.contains("data-testid=\"rating-meta\""));
        assert!(html.contains("Not rated yet"));
        // Half-star click targets carry accessible labels.
        assert!(html.contains("aria-label=\"Rate 1 star\""));
        assert!(html.contains("aria-label=\"Rate 0.5 stars\""));
    }

    /// One provider's stored rating, for the community-list render tests.
    fn community(
        provider: MetadataProvider,
        score: f64,
        count: Option<i64>,
        url: Option<&str>,
    ) -> ExternalRating {
        ExternalRating::new(
            provider,
            omnibus_shared::external_ratings::ProviderRating::new(
                Some(score),
                5.0,
                count,
                url.map(str::to_string),
            )
            .expect("fixture score is real"),
            0,
        )
    }

    #[test]
    fn community_ratings_render_each_source_with_its_scale_and_count() {
        let html = render(rsx! {
            BdCommunityRatings {
                ratings: vec![
                    community(MetadataProvider::GoogleBooks, 4.2, Some(1_840), Some("https://books.google.com/x")),
                    community(MetadataProvider::OpenLibrary, 3.75, Some(42), None),
                ],
            }
        });

        assert!(html.contains("data-testid=\"community-ratings\""));
        assert!(html.contains("Community ratings"));
        assert!(html.contains("4.2/5 (1,840)"));
        assert!(html.contains("Google Books"));
        assert!(html.contains("3.8/5 (42)"));
        assert!(html.contains("Open Library"));
        // The source link opens out of the app, so it carries the same
        // hardening the suggestion cards do.
        assert!(html.contains("rel=\"noopener noreferrer\""));
    }

    #[test]
    fn community_rating_renders_no_count_when_the_provider_reported_none() {
        // Absent is not zero: "(0)" would read as nobody having rated it.
        let html = render(rsx! {
            BdCommunityRatings {
                ratings: vec![community(MetadataProvider::Hardcover, 4.0, None, None)],
            }
        });

        assert!(html.contains("4/5"));
        assert!(!html.contains("(0)"));
        assert!(
            !html.contains("href"),
            "no source url means no outbound link"
        );
    }
}
