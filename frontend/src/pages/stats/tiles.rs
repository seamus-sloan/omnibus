//! Headline metric tiles for the stats page: Finished (accent), Avg rating,
//! Pages (each finished book's length resolved through the ladder in
//! `db::stats::pages`, which prefers a real print page count and falls back to
//! a word estimate), and Listening. All four are period-scoped and re-render
//! when the switcher changes, and each carries a pull-to-expand grip that
//! opens its drill-in (`drill_in.rs`).

use dioxus::prelude::*;

use super::drill_in::Metric;

/// Value + unit for a duration tile: minutes under an hour, one-decimal
/// hours under ten, whole hours beyond ("42" m · "3.5" h · "142" h).
fn duration_value(secs: i64) -> (String, &'static str) {
    if secs < 3600 {
        return ((secs / 60).to_string(), "m");
    }
    // Display-only: listening totals sit far below f64's 2^52 exact-integer
    // range.
    #[allow(clippy::cast_precision_loss)]
    let hours = secs as f64 / 3600.0;
    if hours < 10.0 {
        (format!("{hours:.1}"), "h")
    } else {
        (format!("{:.0}", hours.floor()), "h")
    }
}

/// One-decimal star mean, or the em-dash empty state — never NaN or 0.0.
/// Rounds half away from zero (explicit `round`) so a quarter-step mean like
/// 4.25 shows as 4.3, not the 4.2 that `{:.1}`'s round-half-to-even yields.
fn avg_stars_value(avg: Option<f64>) -> String {
    match avg {
        Some(stars) => format!("{:.1}", (stars * 10.0).round() / 10.0),
        None => "\u{2014}".to_string(),
    }
}

/// Thousand-grouped page count ("9,214"), or the em-dash empty state when
/// [`omnibus_shared::StatsSummary::pages_read`] is `None` — no finished book
/// in the window resolved a length on any rung of the ladder (AC2).
fn pages_value(pages: Option<i64>) -> String {
    match pages {
        Some(n) => group_thousands(n),
        None => "\u{2014}".to_string(),
    }
}

/// Group a non-negative integer's digits in threes with `,` separators.
/// Negative inputs (never produced by `db::stats::pages`) fall back to a
/// plain `to_string` rather than mangling the sign.
fn group_thousands(n: i64) -> String {
    if n < 0 {
        return n.to_string();
    }
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// The four-tile headline row. Finished carries the accent tint; Pages is a
/// resolved book length rather than a real page position — exact for a comic
/// or a print-edition override, an estimate otherwise — so its label is
/// explicit about that. Takes only the scalar fields it
/// renders — never the whole `StatsSummary` — so a re-render doesn't clone
/// the DTO's heatmap / finished-books vectors. `expanded` is set by a
/// tile's grip to open its drill-in (`drill_in.rs`).
#[component]
pub(super) fn HeadlineTiles(
    books_finished: i64,
    avg_stars: Option<f64>,
    pages_read: Option<i64>,
    listening_seconds: i64,
    expanded: Signal<Option<Metric>>,
) -> Element {
    let finished = books_finished.to_string();
    let avg = avg_stars_value(avg_stars);
    let has_avg = avg_stars.is_some();
    let pages = pages_value(pages_read);
    let (listen_value, listen_unit) = duration_value(listening_seconds);
    rsx! {
        div { class: "st-tiles",
            StatTile {
                value: finished,
                unit: None,
                meta: StatTileMeta {
                    label: "Finished",
                    accent: true,
                    hero: true,
                    testid: "stats-tile-finished",
                },
                onexpand: move |_| expanded.set(Some(Metric::Finished)),
            }
            StatTile {
                value: avg,
                unit: if has_avg { Some(" \u{2605}".to_string()) } else { None },
                meta: StatTileMeta {
                    label: "Avg rating",
                    accent: false,
                    hero: true,
                    testid: "stats-tile-avg-rating",
                },
                onexpand: move |_| expanded.set(Some(Metric::AvgRating)),
            }
            StatTile {
                value: pages,
                unit: None,
                meta: StatTileMeta {
                    label: "Est. pages",
                    accent: false,
                    hero: false,
                    testid: "stats-tile-pages",
                },
                onexpand: move |_| expanded.set(Some(Metric::Pages)),
            }
            StatTile {
                value: listen_value,
                unit: Some(listen_unit.to_string()),
                meta: StatTileMeta {
                    label: "Listening",
                    accent: false,
                    hero: false,
                    testid: "stats-tile-listening",
                },
                onexpand: move |_| expanded.set(Some(Metric::Listening)),
            }
        }
    }
}

/// Micro-label, accent/hero styling, and testid for one metric tile, grouped
/// to keep [`StatTile`]'s props at the established cap — mirrors
/// `chip_editor::ChipEditorOptions`' presentational-knobs split.
#[derive(Clone, PartialEq)]
struct StatTileMeta {
    label: &'static str,
    accent: bool,
    hero: bool,
    testid: &'static str,
}

/// One metric tile: a big serif number (with an optional smaller unit) over
/// a micro-label, rendered as a button so the whole tile — plus its bottom
/// grip glyph — opens the metric's drill-in on activation. `hero` sizes the
/// phone layout's top pair larger.
#[component]
fn StatTile(
    value: String,
    unit: Option<String>,
    meta: StatTileMeta,
    onexpand: EventHandler<()>,
) -> Element {
    let StatTileMeta {
        label,
        accent,
        hero,
        testid,
    } = meta;
    let mut class = String::from("card st-tile");
    if accent {
        class.push_str(" st-tile-accent");
    }
    if hero {
        class.push_str(" st-tile-hero");
    }
    rsx! {
        button {
            class: "{class}",
            "data-testid": testid,
            r#type: "button",
            "aria-label": "Expand {label} details",
            onclick: move |_| onexpand.call(()),
            // Keyed on the rendered value so a period switch remounts just
            // this node when the new summary lands — replaying the content
            // swap animation while the tile itself stays put (iOS
            // numericText analogue). An unchanged value doesn't animate.
            div { key: "{value}", class: "st-tile-value",
                {value}
                if let Some(u) = unit {
                    span { class: "st-tile-unit", {u} }
                }
            }
            div { class: "label st-tile-label", {label} }
            span { class: "st-tile-grip", aria_hidden: "true" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_value_scales_minutes_decimal_hours_and_whole_hours() {
        assert_eq!(duration_value(0), ("0".into(), "m"));
        assert_eq!(duration_value(42 * 60), ("42".into(), "m"));
        assert_eq!(duration_value(3 * 3600 + 1800), ("3.5".into(), "h"));
        assert_eq!(duration_value(142 * 3600 + 120), ("142".into(), "h"));
    }

    #[test]
    fn avg_stars_value_rounds_half_up_to_one_decimal_or_em_dash() {
        // 4.25 rounds half away from zero → 4.3 (not {:.1}'s half-to-even 4.2).
        assert_eq!(avg_stars_value(Some(4.25)), "4.3");
        assert_eq!(avg_stars_value(Some(4.24)), "4.2");
        assert_eq!(avg_stars_value(Some(5.0)), "5.0");
        assert_eq!(avg_stars_value(None), "\u{2014}");
    }

    #[test]
    fn pages_value_groups_thousands_or_shows_em_dash() {
        assert_eq!(pages_value(Some(0)), "0");
        assert_eq!(pages_value(Some(214)), "214");
        assert_eq!(pages_value(Some(9214)), "9,214");
        assert_eq!(pages_value(Some(1_234_567)), "1,234,567");
        assert_eq!(pages_value(None), "\u{2014}");
    }

    #[test]
    fn group_thousands_handles_short_and_negative_inputs() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(9), "9");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1000), "1,000");
        assert_eq!(group_thousands(-42), "-42");
    }
}
