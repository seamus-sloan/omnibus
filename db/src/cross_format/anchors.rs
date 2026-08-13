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
    chapter_no: Option<u32>,
    synthetic: bool,
    frac: f64,
}

/// An ebook-side candidate: title key plus start as a fraction of the text.
#[derive(Debug, Clone)]
struct TextMark {
    key: Option<String>,
    chapter_no: Option<u32>,
    frac: f64,
}

/// Chapter ordinal parsed from a `Chapter <n>[: subtitle]`-shaped title,
/// arabic or spelled out — so a mark decorated with subtitles ("Chapter
/// One: The Pigeon Drop: In Which…") pairs with a nav's terser "Chapter 1".
pub(super) fn chapter_number(raw: &str) -> Option<u32> {
    let lower = strip_track_prefix(raw).trim_start().to_lowercase();
    let rest = lower.strip_prefix("chapter")?;
    let rest = rest.trim_start_matches([' ', '.', '-', '_', ':']);
    if rest.is_empty() || rest.len() == lower.len() - "chapter".len() {
        // No separator after "chapter" ("chapterhouse") is a word, not a
        // numbering — unless the ordinal follows immediately ("chapter1").
        if !rest.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
    }
    // Unicode hyphen variants fold to ASCII so "twenty‑one" tokenizes as
    // one composite word.
    let cleaned = rest.replace(['\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}'], "-");
    let mut tokens = cleaned
        .split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .filter(|t| !t.is_empty());
    let first = tokens.next()?;
    if let Ok(n) = first.parse::<u32>() {
        return Some(n);
    }
    let value = word_number(first)?;
    // A spaced composite ("Chapter Twenty One") spells its unit as the
    // next token — prefer the combined value over the bare tens.
    if value >= 20 && value % 10 == 0 {
        if let Some(unit) = tokens.next().and_then(word_number).filter(|u| *u < 10) {
            return Some(value + unit);
        }
    }
    Some(value)
}

/// Spelled-out ordinal → number, through ninety-nine ("twenty-one").
fn word_number(word: &str) -> Option<u32> {
    const UNITS: [(&str, u32); 19] = [
        ("one", 1),
        ("two", 2),
        ("three", 3),
        ("four", 4),
        ("five", 5),
        ("six", 6),
        ("seven", 7),
        ("eight", 8),
        ("nine", 9),
        ("ten", 10),
        ("eleven", 11),
        ("twelve", 12),
        ("thirteen", 13),
        ("fourteen", 14),
        ("fifteen", 15),
        ("sixteen", 16),
        ("seventeen", 17),
        ("eighteen", 18),
        ("nineteen", 19),
    ];
    const TENS: [(&str, u32); 8] = [
        ("twenty", 20),
        ("thirty", 30),
        ("forty", 40),
        ("fifty", 50),
        ("sixty", 60),
        ("seventy", 70),
        ("eighty", 80),
        ("ninety", 90),
    ];
    let lookup = |w: &str| UNITS.iter().find(|(n, _)| *n == w).map(|(_, v)| *v);
    if let Some(v) = lookup(word) {
        return Some(v);
    }
    if let Some((tens_word, unit_word)) = word.split_once('-') {
        let tens = TENS
            .iter()
            .find(|(n, _)| *n == tens_word)
            .map(|(_, v)| *v)?;
        return Some(tens + lookup(unit_word).filter(|v| *v < 10)?);
    }
    TENS.iter().find(|(n, _)| *n == word).map(|(_, v)| *v)
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
            chapter_no: chapter_number(&c.title),
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

    // The match bar scores against the *smaller* mark list: a nav padded
    // with front matter (Cover, Copyright, Contents…) must not raise the
    // bar past what the audio side could ever supply (#1894).
    let usable_audio = audio
        .iter()
        .filter(|a| !a.synthetic && a.key.is_some())
        .count();
    let denom = text.len().min(usable_audio.max(1));
    let passes = |a: &[Anchor]| {
        a.len() >= MIN_ANCHORS && (a.len() as f64) >= MIN_MATCH_FRACTION * denom as f64
    };

    // Rungs, most exact first; the first that clears the bar (after the
    // monotonic filter, so the count reflects anchors the interpolation
    // actually uses) wins. Junk encoder labels ("STORMLIGHT0501P01")
    // clear none of them and fall through to an honest refusal.
    let mut chosen: Option<Vec<Anchor>> = None;
    for rung in [
        match_by_title(&text, &audio),
        match_by_chapter_number(&text, &audio),
        match_by_title_prefix(&text, &audio),
    ] {
        let m = monotonic(rung);
        if passes(&m) {
            chosen = Some(m);
            break;
        }
    }
    // Count alignment last: one-to-one lists pair positionally (the
    // per-chapter MP3 folder, or two editions wording titles differently).
    let anchors = match chosen {
        Some(a) => a,
        None if text.len() == audio.len() => {
            let m = monotonic(
                text.iter()
                    .zip(audio.iter())
                    .map(|(t, a)| Anchor {
                        text_frac: t.frac,
                        audio_frac: a.frac,
                    })
                    .collect(),
            );
            if !passes(&m) {
                return Ok(None);
            }
            m
        }
        None => return Ok(None),
    };
    Ok(Some(AnchorMap {
        matched: anchors.len() as i64,
        anchors,
        ebook_chapters: text.len() as i64,
    }))
}

/// Ordered pairing on parsed chapter numbers — the rung that survives
/// subtitle decoration and numbering-style drift ("Chapter One: …" vs
/// "Chapter 1").
fn match_by_chapter_number(text: &[TextMark], audio: &[AudioMark]) -> Vec<Anchor> {
    let mut anchors = Vec::new();
    let mut ai = 0usize;
    for t in text {
        let Some(t_no) = t.chapter_no else { continue };
        let mut j = ai;
        while j < audio.len() {
            let a = &audio[j];
            if !a.synthetic && a.chapter_no == Some(t_no) {
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

/// Minimum normalized-key length before a prefix counts as a match — one
/// short word prefixing another title is coincidence, not correspondence.
/// Multi-word keys qualify shorter ("Day One" ↔ "Day One: …"), since two
/// words already carry structure a lone stem doesn't.
const MIN_PREFIX_CHARS: usize = 8;
const MIN_MULTIWORD_PREFIX_CHARS: usize = 5;

fn keys_prefix_match(a: &str, b: &str) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let qualified = short.len() >= MIN_PREFIX_CHARS
        || (short.contains(' ') && short.len() >= MIN_MULTIWORD_PREFIX_CHARS);
    // The prefix must end on a word boundary in the longer key — "day one"
    // must not pair with "day ones revenge".
    qualified
        && long.starts_with(short)
        && (long.len() == short.len() || long.as_bytes().get(short.len()) == Some(&b' '))
}

/// Ordered pairing where one normalized title is a prefix of the other —
/// the shape of a terse nav entry against a subtitle-decorated audio mark
/// (or vice versa) when neither side carries a chapter number.
fn match_by_title_prefix(text: &[TextMark], audio: &[AudioMark]) -> Vec<Anchor> {
    let mut anchors = Vec::new();
    let mut ai = 0usize;
    for t in text {
        let Some(t_key) = &t.key else { continue };
        let mut j = ai;
        while j < audio.len() {
            let a = &audio[j];
            if !a.synthetic
                && a.key
                    .as_ref()
                    .is_some_and(|a_key| keys_prefix_match(a_key, t_key))
            {
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

/// How many usable (titled, non-synthetic) audio marks the timeline
/// carries — lets the modal distinguish "no marks in the audio" from
/// "marks exist but couldn't be aligned".
pub(super) async fn usable_audio_marks(
    pool: &SqlitePool,
    timeline: &AudioTimeline,
) -> Result<i64, CrossFormatError> {
    Ok(audio_marks(pool, timeline)
        .await?
        .iter()
        .filter(|a| !a.synthetic && a.key.is_some())
        .count() as i64)
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
                    chapter_no: chapter_number(stem),
                    synthetic: false,
                    frac: ((file.start_seconds + start) / timeline.total_seconds).clamp(0.0, 1.0),
                });
                start += part.duration_seconds.max(0.0);
            }
        } else {
            for c in &chapters {
                marks.push(AudioMark {
                    key: title_key(&c.title),
                    chapter_no: chapter_number(&c.title),
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

/// Fold user-declared sync points into the anchor set. User pairs are
/// ground truth: they seed the set, and chapter anchors survive only where
/// they stay strictly monotonic between the user pairs on both axes.
pub(super) fn merge_user_anchors(user: &[Anchor], chapter: Option<AnchorMap>) -> Option<AnchorMap> {
    if user.is_empty() {
        return chapter;
    }
    let mut merged: Vec<Anchor> = user.to_vec();
    merged.sort_by(|a, b| a.text_frac.total_cmp(&b.text_frac));
    let merged = monotonic(merged);
    let (chapter_anchors, matched, ebook_chapters) = match &chapter {
        Some(m) => (m.anchors.as_slice(), m.matched, m.ebook_chapters),
        None => (&[][..], 0, 0),
    };
    let mut out: Vec<Anchor> = merged.clone();
    for c in chapter_anchors {
        // Strictly between its would-be neighbors on both axes, measured
        // against the set built so far.
        let pos = out.partition_point(|a| a.text_frac < c.text_frac);
        let below_ok = pos == 0
            || (out[pos - 1].text_frac < c.text_frac && out[pos - 1].audio_frac < c.audio_frac);
        let above_ok = pos == out.len()
            || (c.text_frac < out[pos].text_frac && c.audio_frac < out[pos].audio_frac);
        if below_ok && above_ok {
            out.insert(pos, *c);
        }
    }
    Some(AnchorMap {
        anchors: out,
        matched,
        ebook_chapters,
    })
}
