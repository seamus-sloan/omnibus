//! Position anchors for PDF pages — the reading position, and the quads a
//! highlight covers — stored in the same text columns an EPUB CFI uses
//! (`reading_progress.epub_cfi`, `annotations.epub_cfi_range`,
//! `bookmarks.position`). Every client round-trips them through these
//! helpers rather than inventing its own encoding, exactly as
//! [`crate::comic_page_anchor`] does for comics.

/// Prefix of a page-position anchor: `pdf-page:N`, 0-based.
pub const PDF_PAGE_ANCHOR_PREFIX: &str = "pdf-page:";
/// Prefix of a highlight anchor: `pdf:{page}:{quads}`.
pub const PDF_HIGHLIGHT_ANCHOR_PREFIX: &str = "pdf:";
/// Most quads one highlight anchor carries. Past this the anchor degrades to
/// the page alone so it stays well under `EPUB_CFI_RANGE_MAX_LEN` — the
/// highlight still lists and jumps, it just is not painted.
pub const PDF_ANCHOR_MAX_QUADS: usize = 60;

/// Position anchor for a PDF page, stored in the `epub_cfi` slot of an
/// `Epub`-format progress row (or a bookmark's `position`). Distinct from the
/// comic prefix so the server can report a PDF page as a spine index — the
/// PDF's structure tables carry one entry per page — where a comic reports
/// none.
pub fn pdf_page_anchor(page: usize) -> String {
    format!("{PDF_PAGE_ANCHOR_PREFIX}{page}")
}

/// Parse a [`pdf_page_anchor`] back to its 0-based page. `None` for anything
/// else — a comic anchor, a real CFI — so callers fall back rather than
/// misread a foreign position.
pub fn parse_pdf_page_anchor(anchor: &str) -> Option<usize> {
    anchor.strip_prefix(PDF_PAGE_ANCHOR_PREFIX)?.parse().ok()
}

/// Whether a stored position is an EPUB CFI at all. The EPUB readers hand a
/// stored anchor straight to epub.js; a `pdf-page:`/`comic-page:` anchor on a
/// book that also carries an EPUB must never reach it.
pub fn is_epub_cfi(anchor: &str) -> bool {
    anchor.trim_start().starts_with("epubcfi(")
}

/// One quadrilateral of a highlight, in PDF user-space points on the
/// unrotated page with the origin at the bottom-left — the coordinate frame
/// both PDFKit (`PDFSelection.bounds(for:)`) and PDF.js (`getTextContent`
/// through a rotation-0 viewport) speak, so an anchor written by one client
/// paints identically on the other. Corner order is the PDF `QuadPoints`
/// convention: upper-left, upper-right, lower-left, lower-right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PdfQuad {
    pub points: [(f32, f32); 4],
}

/// A highlight's anchor: the page it lives on and the quads it covers. An
/// empty `quads` is the degraded page-only form.
#[derive(Debug, Clone, PartialEq)]
pub struct PdfAnchor {
    pub page: usize,
    pub quads: Vec<PdfQuad>,
}

impl PdfAnchor {
    /// Encode as `pdf:{page}:{x1,y1,…,x4,y4};{…}` with one decimal of
    /// precision — sub-tenth-point differences are below any renderer's
    /// hit-test resolution. More than [`PDF_ANCHOR_MAX_QUADS`] quads encodes
    /// as the page alone.
    pub fn encode(&self) -> String {
        if self.quads.is_empty() || self.quads.len() > PDF_ANCHOR_MAX_QUADS {
            return format!("{PDF_HIGHLIGHT_ANCHOR_PREFIX}{}", self.page);
        }
        let quads: Vec<String> = self
            .quads
            .iter()
            .map(|q| {
                q.points
                    .iter()
                    .flat_map(|(x, y)| [format!("{x:.1}"), format!("{y:.1}")])
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect();
        format!(
            "{PDF_HIGHLIGHT_ANCHOR_PREFIX}{}:{}",
            self.page,
            quads.join(";")
        )
    }

    /// Parse an [`encode`](Self::encode)d anchor. `None` for a foreign anchor
    /// (a CFI, a page-position anchor) or a malformed quad list, so a reader
    /// never paints a rectangle it cannot place.
    pub fn parse(anchor: &str) -> Option<Self> {
        let rest = anchor.strip_prefix(PDF_HIGHLIGHT_ANCHOR_PREFIX)?;
        let (page, quads) = match rest.split_once(':') {
            Some((page, quads)) => (page, Some(quads)),
            None => (rest, None),
        };
        let page: usize = page.parse().ok()?;
        let Some(quads) = quads.filter(|q| !q.is_empty()) else {
            return Some(Self {
                page,
                quads: Vec::new(),
            });
        };
        let mut parsed = Vec::new();
        for quad in quads.split(';') {
            let nums: Vec<f32> = quad
                .split(',')
                .map(|n| n.trim().parse::<f32>().ok().filter(|v| v.is_finite()))
                .collect::<Option<Vec<_>>>()?;
            if nums.len() != 8 {
                return None;
            }
            parsed.push(PdfQuad {
                points: [
                    (nums[0], nums[1]),
                    (nums[2], nums[3]),
                    (nums[4], nums[5]),
                    (nums[6], nums[7]),
                ],
            });
        }
        if parsed.len() > PDF_ANCHOR_MAX_QUADS {
            return None;
        }
        Some(Self {
            page,
            quads: parsed,
        })
    }
}

/// The 0-based page named by either PDF anchor form — a position anchor or
/// a highlight anchor — for callers that place both on the page ruler.
pub fn pdf_anchor_page(anchor: &str) -> Option<usize> {
    parse_pdf_page_anchor(anchor).or_else(|| PdfAnchor::parse(anchor).map(|a| a.page))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(x: f32, y: f32) -> PdfQuad {
        PdfQuad {
            points: [(x, y + 10.0), (x + 100.0, y + 10.0), (x, y), (x + 100.0, y)],
        }
    }

    #[test]
    fn pdf_page_anchor_round_trips_and_rejects_foreign_anchors() {
        assert_eq!(pdf_page_anchor(7), "pdf-page:7");
        assert_eq!(parse_pdf_page_anchor("pdf-page:7"), Some(7));
        assert_eq!(parse_pdf_page_anchor("comic-page:7"), None);
        assert_eq!(parse_pdf_page_anchor("epubcfi(/6/4!/4/2)"), None);
        assert_eq!(parse_pdf_page_anchor("pdf-page:-1"), None);
        assert_eq!(parse_pdf_page_anchor("pdf:7"), None);
    }

    #[test]
    fn is_epub_cfi_recognises_only_cfis() {
        assert!(is_epub_cfi("epubcfi(/6/4!/4/2,/1:0,/1:42)"));
        assert!(!is_epub_cfi("pdf-page:3"));
        assert!(!is_epub_cfi("comic-page:3"));
        assert!(!is_epub_cfi("12.5"));
    }

    #[test]
    fn pdf_anchor_encodes_with_one_decimal_and_parses_back() {
        let anchor = PdfAnchor {
            page: 2,
            quads: vec![quad(72.0, 700.25), quad(72.0, 686.04)],
        };
        let encoded = anchor.encode();
        assert_eq!(
            encoded,
            "pdf:2:72.0,710.2,172.0,710.2,72.0,700.2,172.0,700.2;\
             72.0,696.0,172.0,696.0,72.0,686.0,172.0,686.0"
        );
        let parsed = PdfAnchor::parse(&encoded).unwrap();
        assert_eq!(parsed.page, 2);
        assert_eq!(parsed.quads.len(), 2);
        assert_eq!(parsed.quads[0].points[2], (72.0, 700.2));
    }

    #[test]
    fn pdf_anchor_degrades_to_the_page_past_the_quad_cap() {
        let anchor = PdfAnchor {
            page: 4,
            quads: (0..PDF_ANCHOR_MAX_QUADS + 1)
                .map(|i| quad(0.0, i as f32))
                .collect(),
        };
        assert_eq!(anchor.encode(), "pdf:4");
        let parsed = PdfAnchor::parse("pdf:4").unwrap();
        assert_eq!(parsed.page, 4);
        assert!(parsed.quads.is_empty());
    }

    #[test]
    fn pdf_anchor_rejects_malformed_and_foreign_input() {
        assert_eq!(PdfAnchor::parse("pdf-page:4"), None);
        assert_eq!(PdfAnchor::parse("epubcfi(/6/4)"), None);
        assert_eq!(PdfAnchor::parse("pdf:x"), None);
        assert_eq!(PdfAnchor::parse("pdf:1:1,2,3"), None);
        assert_eq!(PdfAnchor::parse("pdf:1:1,2,3,4,5,6,7,NaN"), None);
    }

    #[test]
    fn pdf_anchor_page_reads_both_forms() {
        assert_eq!(pdf_anchor_page("pdf-page:9"), Some(9));
        assert_eq!(pdf_anchor_page("pdf:9"), Some(9));
        assert_eq!(pdf_anchor_page("pdf:9:0,0,1,0,0,1,1,1"), Some(9));
        assert_eq!(pdf_anchor_page("comic-page:9"), None);
    }
}
