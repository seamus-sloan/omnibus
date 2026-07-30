//! Minimal EPUB CFI parser/formatter for reading positions — the
//! `epubcfi(/6/N!/4/…/K:off)` shape epub.js emits on relocate. Range CFIs
//! and temporal/spatial terms are deliberately rejected (`None`): a resume
//! position is always a point, and an unsupported dialect must degrade to
//! percent-only rather than resolve to a wrong location.

/// A parsed point CFI: which spine document, then the path within it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cfi {
    pub spine_index: usize,
    pub tail: CfiTail,
}

/// The intra-document half of a CFI, after the `!` indirection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfiTail {
    /// Even element steps in document order (each `2k` = k-th element child).
    pub element_steps: Vec<u32>,
    /// Final odd step: the coalesced-text-node index among the last
    /// element's children.
    pub text_index: u32,
    /// Character offset into that text node, in UTF-16 code units (the DOM
    /// unit epub.js counts in).
    pub offset_utf16: u32,
}

/// Parse an `epubcfi(…)` string. `None` for anything but a supported point
/// CFI: ranges (`,`), temporal/spatial terms (`~`/`@`), or a package path
/// that isn't the conventional `/6/N` spine step.
pub fn parse_cfi(raw: &str) -> Option<Cfi> {
    let inner = raw.trim().strip_prefix("epubcfi(")?.strip_suffix(')')?;
    if inner.contains(',') || inner.contains('~') || inner.contains('@') {
        return None;
    }
    let (package, tail) = inner.split_once('!')?;
    let package_steps = parse_steps(package)?;
    // The conventional package path is /6 (the spine element) then the
    // itemref step; anything deeper points outside the spine model.
    let [six, item] = package_steps.as_slice() else {
        return None;
    };
    if six.step != 6 || item.step % 2 != 0 || item.step == 0 {
        return None;
    }
    let spine_index = (item.step / 2 - 1) as usize;
    Some(Cfi {
        spine_index,
        tail: parse_tail(tail)?,
    })
}

/// Format a range CFI (`epubcfi(parent,start,end)`) for two tails in the
/// same spine document — the shape epub.js's `annotations.highlight()`
/// resolves. The parent component carries the common element-step prefix,
/// mirroring what epub.js's own `cfiFromRange` emits.
pub fn format_range_cfi(spine_index: usize, start: &CfiTail, end: &CfiTail) -> String {
    let common = start
        .element_steps
        .iter()
        .zip(&end.element_steps)
        .take_while(|(a, b)| a == b)
        .count();
    let rel = |tail: &CfiTail| {
        let mut path = String::new();
        for step in &tail.element_steps[common..] {
            path.push_str(&format!("/{step}"));
        }
        format!("{path}/{}:{}", tail.text_index, tail.offset_utf16)
    };
    let mut parent = String::new();
    for step in &start.element_steps[..common] {
        parent.push_str(&format!("/{step}"));
    }
    format!(
        "epubcfi(/6/{}!{parent},{},{})",
        2 * (spine_index + 1),
        rel(start),
        rel(end)
    )
}

/// Format a point CFI for the given spine index and tail.
pub fn format_cfi(spine_index: usize, tail: &CfiTail) -> String {
    let mut path = String::new();
    for step in &tail.element_steps {
        path.push_str(&format!("/{step}"));
    }
    format!(
        "epubcfi(/6/{}!{path}/{}:{})",
        2 * (spine_index + 1),
        tail.text_index,
        tail.offset_utf16
    )
}

struct Step {
    step: u32,
    offset: Option<u32>,
}

fn parse_steps(path: &str) -> Option<Vec<Step>> {
    let mut steps = Vec::new();
    for part in path.split('/') {
        if part.is_empty() {
            continue;
        }
        // Strip an `[idAssertion]` (epub.js emits them on some steps); the
        // numeric step alone is authoritative for our resolution.
        let part = match part.split_once('[') {
            Some((head, rest)) if rest.ends_with(']') => head,
            Some(_) => return None,
            None => part,
        };
        let (num, offset) = match part.split_once(':') {
            Some((n, off)) => (n, Some(off.parse().ok()?)),
            None => (part, None),
        };
        steps.push(Step {
            step: num.parse().ok()?,
            offset,
        });
    }
    Some(steps)
}

fn parse_tail(tail: &str) -> Option<CfiTail> {
    let steps = parse_steps(tail)?;
    // Only the terminal step may carry a `:offset`.
    if steps
        .iter()
        .rev()
        .skip(1)
        .any(|s| s.offset.is_some() || s.step % 2 != 0)
    {
        return None;
    }
    let last = steps.last()?;
    if last.step % 2 == 1 {
        let element_steps = steps[..steps.len() - 1].iter().map(|s| s.step).collect();
        Some(CfiTail {
            element_steps,
            text_index: last.step,
            offset_utf16: last.offset.unwrap_or(0),
        })
    } else {
        // Element-anchored CFI (no text step): resolve to the element's
        // first text position.
        Some(CfiTail {
            element_steps: steps.iter().map(|s| s.step).collect(),
            text_index: 1,
            offset_utf16: 0,
        })
    }
}
