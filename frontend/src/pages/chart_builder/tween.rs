//! Value interpolation between two results, so a selection change moves the
//! marks rather than swapping them.
//!
//! **Values are what's interpolated, never geometry.** Bars, dots, the line
//! and its area all derive their shape from the same `values` arrays, so
//! tweening those makes every mark move together from one mechanism — where a
//! CSS transition on `y`/`height` would animate the bars while the line's
//! `d` (which CSS cannot interpolate) jumped underneath them.
//!
//! **Axes snap rather than tween**, deliberately. A lerped maximum drags its
//! tick labels through 3.7, 7.4 and the rest; the ticks are meant to be round
//! numbers, so the scale changes at once and the marks travel to their places
//! on it.
//!
//! Marks are matched **by identity, not by position**: a series by its measure
//! and slice, a value by its bucket key. That is what makes the motion mean
//! something — regrouping, zooming or toggling a measure all rebuild the
//! arrays, and a positional blend would interpolate March into April. Matched
//! marks travel; unmatched ones grow in from the baseline, which is the same
//! motion the entrance animation plays for a chart's first paint.

use dioxus::prelude::*;
use omnibus_shared::{ChartResult, ChartSeries};

use crate::platform_sleep::async_sleep_ms;

/// Frames the tween is cut into. At [`FRAME_MS`] apiece this is the whole
/// duration; more frames buy smoothness at the cost of a re-render each.
const STEPS: u32 = 22;
/// Milliseconds between frames — roughly a 60Hz budget.
const FRAME_MS: u32 = 16;
/// Above this many buckets the tween is skipped entirely: re-rendering
/// hundreds of marks twenty-two times costs more than the motion is worth,
/// and at that density individual bars are a pixel wide anyway.
const MAX_TWEEN_BUCKETS: usize = 120;

/// Ease-out cubic — fast departure, settled arrival. Matches the `--cb-ease`
/// curve the entrance animation uses, so entering and updating marks feel
/// like the same surface.
fn ease_out(t: f64) -> f64 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

/// The value `series` held in the bucket keyed `key`, if it held one.
///
/// Keyed rather than indexed because the arrays are rebuilt on every change:
/// index 3 is March in one result and April in the next, and blending those
/// together would animate a number that never existed.
fn prior_value(from: &ChartResult, series: &ChartSeries, key: &str) -> Option<f64> {
    let matched = from
        .series
        .iter()
        .find(|s| s.measure == series.measure && s.slice == series.slice)?;
    let at = from.buckets.iter().position(|b| b == key)?;
    matched.values.get(at).copied().flatten()
}

/// `from` and `to` blended at `t`, with the axes already on `to`.
///
/// A mark with no counterpart in `from` — a new bucket, a measure just
/// ticked on — grows in from the baseline rather than appearing at full
/// height, which is the same motion a first paint plays.
pub fn blend(from: &ChartResult, to: &ChartResult, t: f64) -> ChartResult {
    let e = ease_out(t);
    let mut out = to.clone();
    for series in out.series.iter_mut() {
        let identity = series.clone();
        for (b, value) in series.values.iter_mut().enumerate() {
            let Some(target) = *value else { continue };
            let Some(key) = to.buckets.get(b) else {
                continue;
            };
            // Absent on the old chart means entering, and an entering mark
            // rises from zero.
            let start = prior_value(from, &identity, key).unwrap_or(0.0);
            *value = Some(start + (target - start) * e);
        }
    }
    out
}

/// A view of `target` that travels to each new value instead of jumping.
///
/// Falls back to `target` whenever no frame has been produced yet, which
/// matters twice: the driving effect does not run during SSR, so the server
/// renders the real data rather than nothing (rule 07); and on the client the
/// first paint after a result arrives is the data itself rather than a blank
/// frame waiting for the effect to catch up.
pub fn use_tweened(target: Memo<Option<ChartResult>>) -> Memo<Option<ChartResult>> {
    let mut shown: Signal<Option<ChartResult>> = use_signal(|| None);
    // Supersedes an in-flight tween the way the fetch effect supersedes a
    // stale request — without it, two quick selections interleave their
    // frames and the marks judder between two destinations.
    let mut epoch = use_signal(|| 0u64);

    use_effect(move || {
        let Some(next) = target() else {
            shown.set(None);
            return;
        };
        let ticket = *epoch.peek() + 1;
        epoch.set(ticket);

        // Nothing on screen yet means a first paint, which the CSS entrance
        // animation owns; past the bucket cap the motion costs more than it
        // is worth. Everything else tweens.
        let previous = shown.peek().clone();
        let Some(from) = previous.filter(|_| next.buckets.len() <= MAX_TWEEN_BUCKETS) else {
            shown.set(Some(next));
            return;
        };

        spawn(async move {
            for step in 1..=STEPS {
                async_sleep_ms(FRAME_MS).await;
                if *epoch.peek() != ticket {
                    return;
                }
                let t = f64::from(step) / f64::from(STEPS);
                shown.set(Some(blend(&from, &next, t)));
            }
        });
    });

    use_memo(move || match target() {
        // A cleared target clears the frame, so an error state can't leave a
        // stale chart standing.
        None => None,
        Some(t) => shown().or(Some(t)),
    })
}

#[cfg(all(test, feature = "server"))]
mod tests;
