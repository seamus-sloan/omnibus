//! The Info-dict fields the indexer consumes, decoded from PDF text strings.
//! Everything else in the dict (dates, producer) is ignored.

use hayro::hayro_syntax::Pdf;

/// Decoded Info-dict metadata. `keywords` is the `/Keywords` string split
/// into tags, since it is the one field the spec leaves as free-form prose.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PdfInfo {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Vec<String>,
}

pub(super) fn read_info(pdf: &Pdf) -> PdfInfo {
    let m = pdf.metadata();
    let decode = |field: &Option<Vec<u8>>| field.as_deref().and_then(decode_pdf_string);
    PdfInfo {
        title: decode(&m.title),
        author: decode(&m.author),
        subject: decode(&m.subject),
        keywords: decode(&m.keywords)
            .map(|k| split_keywords(&k))
            .unwrap_or_default(),
    }
}

/// Decode a PDF text string: UTF-16BE behind a `FE FF` byte-order mark,
/// UTF-8 behind `EF BB BF`, else PDFDocEncoding — which is Latin-1 for every
/// printable character a real document uses. Trimmed; empty reads as `None`.
pub fn decode_pdf_string(bytes: &[u8]) -> Option<String> {
    let text = if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(rest).into_owned()
    } else {
        bytes.iter().map(|&b| char::from(b)).collect()
    };
    let trimmed = text.trim().replace('\0', "");
    (!trimmed.is_empty()).then_some(trimmed)
}

fn split_keywords(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in raw.split([',', ';']) {
        let tag = part.trim();
        if tag.is_empty() || out.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            continue;
        }
        out.push(tag.to_string());
    }
    out
}
