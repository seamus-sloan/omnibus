//! Client-side zoom: narrowing the chart to a sub-range of the buckets the
//! server already sent.
//!
//! **A zoom is stored as a pair of bucket *keys*, never indices.** Change the
//! grouping from monthly to weekly and index 3 silently means a different
//! week; a key that is no longer on the axis simply fails to resolve, and the
//! zoom drops itself rather than framing the wrong stretch of someone's
//! reading.
//!
//! The narrowed slice is re-fitted with `omnibus_shared::chart::fit_axes` —
//! the same assembly the server ran for the full window — so zooming in
//! rescales the y-axis to what is actually on screen instead of leaving a
//! quiet stretch squashed against the floor.
//!
//! **What this cannot do**, and it is the honest limit of a client-only zoom:
//! it can only ever narrow what was already fetched. It never asks for finer
//! buckets, so zooming into three months of a monthly chart shows three wide
//! bars rather than resolving into ninety days, and it cannot reach past a
//! `truncated` axis to the buckets the server clipped. Both need the spec to
//! carry a window the server queries against.

use omnibus_shared::{chart::fit_axes, ChartResult};

/// The fewest buckets a brush may select. A one-bucket "range" is a click, not
/// a selection, and zooming to it would leave a single bar filling the frame.
pub const MIN_BRUSH_BUCKETS: usize = 2;

/// A zoom, as the first and last bucket key it spans.
pub type ZoomRange = (String, String);

/// Resolve a zoom to inclusive indices into `buckets`.
///
/// `None` when either key has left the axis — which is exactly what happens
/// when the grouping or period changes underneath a zoom, and is why the
/// range is stored as keys.
pub fn resolve(buckets: &[String], range: &ZoomRange) -> Option<(usize, usize)> {
    let start = buckets.iter().position(|b| *b == range.0)?;
    let end = buckets.iter().position(|b| *b == range.1)?;
    let (lo, hi) = if start <= end {
        (start, end)
    } else {
        (end, start)
    };
    (hi > lo).then_some((lo, hi))
}

/// `result` narrowed to `range`, with its axes re-fitted to the slice.
///
/// Returns the result untouched when the range doesn't resolve, so a stale
/// zoom degrades to the full view rather than to an error.
pub fn apply(result: &ChartResult, range: Option<&ZoomRange>) -> ChartResult {
    let Some((lo, hi)) = range.and_then(|r| resolve(&result.buckets, r)) else {
        return result.clone();
    };

    let mut out = result.clone();
    out.buckets = result.buckets[lo..=hi].to_vec();
    for (i, series) in out.series.iter_mut().enumerate() {
        series.values = result.series[i].values[lo..=hi].to_vec();
    }
    // Re-fit rather than inherit: an axis sized for the whole range leaves a
    // zoomed-in quiet stretch flat against the floor, which is the opposite of
    // what zooming in is for.
    let (axes, divisions) = fit_axes(&out.series, out.stacked, out.buckets.len());
    out.axes = axes;
    out.divisions = divisions as u8;
    // The clip is a fact about the *fetch*, not about this view, so it travels
    // with the full result and is reported whether or not a zoom is active.
    out
}

/// The keys a brush from `a` to `b` selects, or `None` when the span is too
/// short to be a selection rather than a click.
pub fn brush_range(buckets: &[String], a: usize, b: usize) -> Option<ZoomRange> {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    if hi - lo + 1 < MIN_BRUSH_BUCKETS {
        return None;
    }
    Some((buckets.get(lo)?.clone(), buckets.get(hi)?.clone()))
}

#[cfg(all(test, feature = "server"))]
mod tests;
