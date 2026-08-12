//! Chapter-anchored mapping tier: match the ebook's TOC against the audio
//! chapter marks (or per-chapter MP3 filename stems), then interpolate
//! piecewise through the matched anchor pairs. Every guard degrades to the
//! linear tier — a sparse or non-monotonic match must never produce a
//! confidently wrong map.

use sqlx::SqlitePool;

use super::{AudioTimeline, CrossFormatError};

/// One matched anchor: the same narrative point as a fraction through the
/// ebook text and through the audio timeline. `(0,0)` and `(1,1)` endpoints
/// are implicit in the interpolation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    pub text_frac: f64,
    pub audio_frac: f64,
}

/// A usable anchor map plus the match statistics the modal reports.
#[derive(Debug, Clone, PartialEq)]
pub struct AnchorMap {
    pub anchors: Vec<Anchor>,
    pub matched: i64,
    pub ebook_chapters: i64,
}

/// Minimum anchors and minimum fraction of ebook chapters that must match
/// before the anchored tier engages — below either, linear is more honest.
const MIN_ANCHORS: usize = 2;
const MIN_MATCH_FRACTION: f64 = 0.5;

/// The indexer's synthetic no-chapter fallback ("Part 1", "Part 2", …).
/// Structural boundaries only — never title-matched.
pub(super) fn is_synthetic_title(title: &str) -> bool {
    let Some(rest) = title.strip_prefix("Part ") else {
        return false;
    };
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

/// Normalized comparison key for chapter titles and filename stems: the
/// same fold the library's (title, author) matching uses, so "The
/// Vanishing" matches "03 - the vanishing".
fn title_key(raw: &str) -> Option<String> {
    crate::normalize::normalize_title(strip_track_prefix(raw))
}

/// Drop a leading track/ordinal prefix ("03 - ", "12.", "07_") from a
/// filename stem or chapter title.
fn strip_track_prefix(raw: &str) -> &str {
    let trimmed = raw.trim_start();
    let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return raw;
    }
    trimmed[digits..].trim_start_matches([' ', '-', '_', '.', ':'])
}

/// An audio-side anchor candidate: a title-ish label plus its start as a
/// fraction of the timeline.
#[derive(Debug, Clone)]
struct AudioMark {
    key: Option<String>,
    synthetic: bool,
    frac: f64,
}

/// An ebook-side candidate: title key plus start as a fraction of the text.
#[derive(Debug, Clone)]
struct TextMark {
    key: Option<String>,
    frac: f64,
}

/// Build the anchor map for a linked book, or `None` when no trustworthy
/// anchoring exists. `ebook_file_id` is the served EPUB's `book_files` row
/// (spine stats + chapters live under it).
pub(super) async fn anchor_map(
    pool: &SqlitePool,
    ebook_file_id: i64,
    timeline: &AudioTimeline,
) -> Result<Option<AnchorMap>, CrossFormatError> {
    let stats = crate::epub_structure::get_spine_stats(pool, ebook_file_id).await?;
    let total_chars: i64 = stats
        .iter()
        .fold(0i64, |acc, s| acc.saturating_add(s.visible_chars.max(0)));
    if total_chars <= 0 || timeline.total_seconds <= 0.0 {
        return Ok(None);
    }
    let text: Vec<TextMark> = crate::epub_structure::get_chapters(pool, ebook_file_id)
        .await?
        .into_iter()
        .map(|c| TextMark {
            key: title_key(&c.title),
            frac: (c.start_chars.max(0) as f64 / total_chars as f64).clamp(0.0, 1.0),
        })
        .collect();
    if text.is_empty() {
        return Ok(None);
    }

    let audio = audio_marks(pool, timeline).await?;
    if audio.is_empty() {
        return Ok(None);
    }

    // Rung 1: normalized title equality (synthetic titles excluded), taken
    // in order so a duplicated chapter name pairs positionally.
    let mut anchors = match_by_title(&text, &audio);

    // Rung 2: count alignment — when every chapter lines up one-to-one
    // (the per-chapter MP3 folder, or two editions wording titles
    // differently), pair by index instead.
    if anchors.len() < MIN_ANCHORS && text.len() == audio.len() {
        anchors = text
            .iter()
            .zip(audio.iter())
            .map(|(t, a)| Anchor {
                text_frac: t.frac,
                audio_frac: a.frac,
            })
            .collect();
    }

    // Count after the monotonic filter so the modal's "X of Y matched"
    // reflects the anchors the interpolation actually uses.
    let anchors = monotonic(anchors);
    if anchors.len() < MIN_ANCHORS
        || (anchors.len() as f64) < MIN_MATCH_FRACTION * text.len() as f64
    {
        return Ok(None);
    }
    Ok(Some(AnchorMap {
        matched: anchors.len() as i64,
        anchors,
        ebook_chapters: text.len() as i64,
    }))
}

/// Audio anchor candidates across the timeline: real chapter marks carry
/// their titles; a file whose chapters are all synthetic falls back to its
/// parts' filename stems (the per-chapter MP3 folder shape), which carry
/// the real chapter names when anything does.
async fn audio_marks(
    pool: &SqlitePool,
    timeline: &AudioTimeline,
) -> Result<Vec<AudioMark>, CrossFormatError> {
    let mut marks = Vec::new();
    for file in &timeline.files {
        let chapters = crate::hls::get_chapters(pool, file.book_file_id).await?;
        let all_synthetic =
            !chapters.is_empty() && chapters.iter().all(|c| is_synthetic_title(&c.title));
        if all_synthetic {
            let parts = crate::hls::get_parts(pool, file.book_file_id).await?;
            let mut start = 0.0f64;
            for part in &parts {
                let stem = part
                    .filename
                    .rsplit('/')
                    .next()
                    .and_then(|name| name.rsplit_once('.').map(|(s, _)| s).or(Some(name)))
                    .unwrap_or(&part.filename);
                marks.push(AudioMark {
                    key: title_key(stem),
                    synthetic: false,
                    frac: ((file.start_seconds + start) / timeline.total_seconds).clamp(0.0, 1.0),
                });
                start += part.duration_seconds.max(0.0);
            }
        } else {
            for c in &chapters {
                marks.push(AudioMark {
                    key: title_key(&c.title),
                    synthetic: is_synthetic_title(&c.title),
                    frac: ((file.start_seconds + c.start_seconds) / timeline.total_seconds)
                        .clamp(0.0, 1.0),
                });
            }
        }
    }
    Ok(marks)
}

/// Ordered title matching: walk both mark lists front to back, pairing
/// equal keys — order-preserving by construction, tolerant of unmatched
/// entries on either side. Synthetic audio titles never match.
fn match_by_title(text: &[TextMark], audio: &[AudioMark]) -> Vec<Anchor> {
    let mut anchors = Vec::new();
    let mut ai = 0usize;
    for t in text {
        let Some(t_key) = &t.key else { continue };
        let mut j = ai;
        while j < audio.len() {
            let a = &audio[j];
            if !a.synthetic && a.key.as_ref() == Some(t_key) {
                anchors.push(Anchor {
                    text_frac: t.frac,
                    audio_frac: a.frac,
                });
                ai = j + 1;
                break;
            }
            j += 1;
        }
    }
    anchors
}

/// Keep only anchors strictly increasing on both axes — a crossed pair is
/// a mismatch, and interpolating through it would invert a whole span.
fn monotonic(anchors: Vec<Anchor>) -> Vec<Anchor> {
    let mut out: Vec<Anchor> = Vec::with_capacity(anchors.len());
    for a in anchors {
        if let Some(last) = out.last() {
            if a.text_frac <= last.text_frac || a.audio_frac <= last.audio_frac {
                continue;
            }
        }
        out.push(a);
    }
    out
}

/// Piecewise-linear map of a fraction through the anchor pairs (implicit
/// `(0,0)`/`(1,1)` endpoints). `axis_text_to_audio` picks the direction.
pub(super) fn interpolate(anchors: &[Anchor], frac: f64, text_to_audio: bool) -> f64 {
    let frac = frac.clamp(0.0, 1.0);
    let mut prev = (0.0f64, 0.0f64);
    for a in anchors.iter().chain(std::iter::once(&Anchor {
        text_frac: 1.0,
        audio_frac: 1.0,
    })) {
        let (from, to) = if text_to_audio {
            (a.text_frac, a.audio_frac)
        } else {
            (a.audio_frac, a.text_frac)
        };
        let (pf, pt) = if text_to_audio {
            (prev.0, prev.1)
        } else {
            (prev.1, prev.0)
        };
        if frac <= from {
            if (from - pf).abs() < f64::EPSILON {
                return pt;
            }
            let t = (frac - pf) / (from - pf);
            return (pt + t * (to - pt)).clamp(0.0, 1.0);
        }
        prev = (a.text_frac, a.audio_frac);
    }
    frac
}
