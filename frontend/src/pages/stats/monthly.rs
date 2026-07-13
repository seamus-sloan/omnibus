//! Books-per-month trend chart: a pure-CSS bar chart over the trailing
//! 12-month window baked into `StatsSummary.books_per_month` (all-time
//! section, never re-queried by the switcher). The current (last) month is
//! highlighted with the accent token; the header carries the per-month
//! average annotation.

use dioxus::prelude::*;
use omnibus_shared::{MonthCount, StatsSummary};

/// One rendered bar: month initial, raw count, height relative to the
/// window's tallest month (0..=100), and whether it's the trailing
/// (current) month.
struct MonthBar {
    label: char,
    books: i64,
    height_pct: u32,
    current: bool,
}

/// First letter of a `YYYY-MM` string's month name, `'?'` when malformed —
/// defensive only, the DTO always sends a well-formed month.
fn month_initial(month: &str) -> char {
    const INITIALS: [char; 12] = ['J', 'F', 'M', 'A', 'M', 'J', 'J', 'A', 'S', 'O', 'N', 'D'];
    month
        .rsplit('-')
        .next()
        .and_then(|m| m.parse::<usize>().ok())
        .filter(|m| (1..=12).contains(m))
        .map(|m| INITIALS[m - 1])
        .unwrap_or('?')
}

/// Bar heights scaled to the tallest month in the window; an all-zero window
/// stays all zero rather than dividing by zero. The last entry is always the
/// current month — `books_per_month` is generated ending there.
fn build_bars(months: &[MonthCount]) -> Vec<MonthBar> {
    let max = months.iter().map(|m| m.books).max().unwrap_or(0);
    months
        .iter()
        .enumerate()
        .map(|(i, m)| MonthBar {
            label: month_initial(&m.month),
            books: m.books,
            height_pct: if max <= 0 {
                0
            } else {
                (m.books.max(0) * 100 / max) as u32
            },
            current: i + 1 == months.len(),
        })
        .collect()
}

/// Mean books finished per month over the window, `None` when the window
/// carries no months (defensive — the DTO always sends 12).
fn monthly_average(months: &[MonthCount]) -> Option<f64> {
    if months.is_empty() {
        return None;
    }
    let total: i64 = months.iter().map(|m| m.books).sum();
    Some(total as f64 / months.len() as f64)
}

/// The all-time books-per-month card: header with the per-month average,
/// then 12 CSS bar columns each labeled with its month initial.
#[component]
pub(super) fn MonthlyChart(summary: StatsSummary) -> Element {
    let months = &summary.books_per_month;
    if months.is_empty() {
        return rsx! { div { class: "card st-card-placeholder", aria_hidden: "true" } };
    }
    let bars = build_bars(months);
    let avg_label = match monthly_average(months) {
        Some(avg) => format!("avg {avg:.1}"),
        None => String::new(),
    };

    rsx! {
        div { class: "card st-monthly-card", "data-testid": "stats-monthly-chart",
            div { class: "st-monthly-head",
                div { class: "label", "Books finished \u{00B7} trailing 12 months" }
                div { class: "st-monthly-avg mono", {avg_label} }
            }
            div {
                class: "st-monthly-bars",
                role: "img",
                aria_label: "Books finished per month, trailing 12 months",
                for bar in &bars {
                    div {
                        class: if bar.current { "st-mo-col st-mo-current" } else { "st-mo-col" },
                        "data-testid": "stats-monthly-bar",
                        title: "{bar.books} finished",
                        div { class: "st-mo-track",
                            div { class: "st-mo-bar", style: "height: {bar.height_pct}%;" }
                        }
                        div { class: "st-mo-label", "{bar.label}" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn month(m: &str, books: i64) -> MonthCount {
        MonthCount {
            month: m.into(),
            books,
        }
    }

    #[test]
    fn month_initial_maps_month_numbers_and_falls_back_on_garbage() {
        assert_eq!(month_initial("2026-01"), 'J');
        assert_eq!(month_initial("2026-07"), 'J');
        assert_eq!(month_initial("2026-12"), 'D');
        assert_eq!(month_initial("not-a-month"), '?');
        assert_eq!(month_initial("2026-13"), '?');
    }

    #[test]
    fn build_bars_scales_to_the_tallest_month_and_flags_the_last_as_current() {
        let months = vec![
            month("2026-01", 0),
            month("2026-02", 2),
            month("2026-03", 4),
        ];
        let bars = build_bars(&months);
        assert_eq!(bars[0].height_pct, 0);
        assert_eq!(bars[1].height_pct, 50);
        assert_eq!(bars[2].height_pct, 100);
        assert!(!bars[0].current);
        assert!(!bars[1].current);
        assert!(bars[2].current);
    }

    #[test]
    fn build_bars_stays_zero_when_every_month_is_zero() {
        let months = vec![month("2026-01", 0), month("2026-02", 0)];
        let bars = build_bars(&months);
        assert!(bars.iter().all(|b| b.height_pct == 0));
    }

    #[test]
    fn monthly_average_means_the_window_and_is_none_when_empty() {
        let months = vec![
            month("2026-01", 1),
            month("2026-02", 2),
            month("2026-03", 3),
        ];
        assert_eq!(monthly_average(&months), Some(2.0));
        assert_eq!(monthly_average(&[]), None);
    }
}
