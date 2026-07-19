//! Chapter-position math shared by the full player, the mini-dock, and
//! bookmarks: resolving the current chapter from the elapsed position and
//! computing the seek targets for whole-chapter prev/next jumps.

#![cfg(not(feature = "mobile"))]

use omnibus_shared::ChapterInfo;

/// Derive the current chapter index from `elapsed` and a sorted chapter list.
/// Returns 0 when `chapters` is empty.
pub(super) fn chapter_index_for_elapsed(chapters: &[ChapterInfo], elapsed: f64) -> usize {
    if chapters.is_empty() {
        return 0;
    }
    chapters
        .partition_point(|c| c.start_seconds <= elapsed)
        .saturating_sub(1)
}

/// Resolve the seek target for "previous chapter":
/// - If we're more than 3 s into the current chapter, seek to its start.
/// - If within 3 s of the start and not the first chapter, go to the previous.
/// - Otherwise seek to 0.
///
/// Returns `None` when `chapters` is empty or `idx` is out of bounds
/// (the click handler computes `idx` from a separate signal, so a chapter
/// list refresh in between can leave it stale).
pub(super) fn chapter_prev_target(
    chapters: &[ChapterInfo],
    elapsed: f64,
    idx: usize,
) -> Option<f64> {
    let current = chapters.get(idx)?;
    let target = if elapsed - current.start_seconds > 3.0 {
        current.start_seconds
    } else if let Some(prev) = idx.checked_sub(1).and_then(|i| chapters.get(i)) {
        prev.start_seconds
    } else {
        0.0
    };
    Some(target)
}

/// Resolve the seek target for "next chapter": the start of chapter `idx + 1`.
/// Returns `None` on the last chapter, or when `idx` is out of bounds (same
/// stale-signal caveat as [`chapter_prev_target`]).
pub(super) fn chapter_next_target(chapters: &[ChapterInfo], idx: usize) -> Option<f64> {
    chapters.get(idx + 1).map(|next| next.start_seconds)
}

#[cfg(test)]
mod tests {
    use omnibus_shared::ChapterInfo;

    use super::*;

    fn ch(ordinal: i64, title: &str, start: f64, dur: f64) -> ChapterInfo {
        ChapterInfo {
            ordinal,
            title: title.into(),
            start_seconds: start,
            duration_seconds: dur,
        }
    }

    #[test]
    fn chapter_index_for_elapsed_returns_zero_for_empty_list() {
        assert_eq!(chapter_index_for_elapsed(&[], 60.0), 0);
    }

    #[test]
    fn chapter_index_for_elapsed_returns_first_chapter_before_any_start() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
        assert_eq!(chapter_index_for_elapsed(&chs, 0.0), 0);
        assert_eq!(chapter_index_for_elapsed(&chs, 150.0), 0);
    }

    #[test]
    fn chapter_index_for_elapsed_advances_past_chapter_boundary() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
        assert_eq!(chapter_index_for_elapsed(&chs, 300.0), 1);
        assert_eq!(chapter_index_for_elapsed(&chs, 500.0), 1);
    }

    #[test]
    fn chapter_prev_target_returns_none_when_chapters_empty() {
        assert_eq!(chapter_prev_target(&[], 10.0, 0), None);
    }

    #[test]
    fn chapter_prev_target_returns_chapter_start_when_well_into_chapter() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
        // 50 s into chapter 1 (idx=1, start=300), elapsed=350 → 350-300=50 > 3 → back to 300
        assert_eq!(chapter_prev_target(&chs, 350.0, 1), Some(300.0));
    }

    #[test]
    fn chapter_prev_target_returns_previous_chapter_when_near_start() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
        // 1 s into chapter 1 (idx=1, start=300), elapsed=301 → 301-300=1 ≤ 3 → go to ch 0 start=0
        assert_eq!(chapter_prev_target(&chs, 301.0, 1), Some(0.0));
    }

    #[test]
    fn chapter_prev_target_returns_zero_when_at_first_chapter_start() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0)];
        assert_eq!(chapter_prev_target(&chs, 1.0, 0), Some(0.0));
    }

    #[test]
    fn chapter_prev_target_returns_none_when_idx_is_out_of_bounds() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0)];
        // idx came from a stale signal — chapters list shrunk under it.
        assert_eq!(chapter_prev_target(&chs, 1.0, 5), None);
    }

    #[test]
    fn chapter_next_target_returns_next_chapter_start_mid_book() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
        assert_eq!(chapter_next_target(&chs, 0), Some(300.0));
    }

    #[test]
    fn chapter_next_target_returns_none_on_last_chapter() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
        assert_eq!(chapter_next_target(&chs, 1), None);
    }

    #[test]
    fn chapter_next_target_returns_none_for_empty_or_stale_index() {
        assert_eq!(chapter_next_target(&[], 0), None);
        let chs = vec![ch(1, "Intro", 0.0, 300.0)];
        assert_eq!(chapter_next_target(&chs, 5), None);
    }
}
