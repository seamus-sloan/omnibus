//! Hand-rolled Atom/OPDS 1.2 XML serialization. The shapes this module emits
//! (one `<feed>` of `<entry>` elements, each with a handful of `<link>`s) are
//! simple enough that a small escaping-safe writer is easier to audit than
//! wiring up a generic XML crate — see the [`super`] module doc for the fuller
//! rationale.

/// MIME type for an OPDS navigation feed (links to other feeds).
pub const NAVIGATION_TYPE: &str =
    "application/atom+xml;charset=utf-8;profile=opds-catalog;kind=navigation";
/// MIME type for an OPDS acquisition feed (links to downloadable content).
pub const ACQUISITION_TYPE: &str =
    "application/atom+xml;charset=utf-8;profile=opds-catalog;kind=acquisition";
/// MIME type for the OpenSearch description document (`/opds/osd`).
pub const OPENSEARCH_TYPE: &str = "application/opensearchdescription+xml;charset=utf-8";

/// One `<link>` element.
#[derive(Debug, Clone)]
pub struct Link {
    pub rel: &'static str,
    pub href: String,
    pub type_: &'static str,
}

impl Link {
    /// Build a link. `href` is expected root-relative (`/opds/...`,
    /// `/api/...`) — clients resolve it against the feed's own URL, so
    /// this module never needs to reconstruct the request's origin.
    pub fn new(rel: &'static str, href: impl Into<String>, type_: &'static str) -> Self {
        Self {
            rel,
            href: href.into(),
            type_,
        }
    }

    fn write(&self, out: &mut String) {
        out.push_str("<link rel=\"");
        out.push_str(&escape(self.rel));
        out.push_str("\" href=\"");
        out.push_str(&escape(&self.href));
        out.push_str("\" type=\"");
        out.push_str(&escape(self.type_));
        out.push_str("\"/>");
    }
}

/// One `<entry>` — a navigation link (folder, e.g. a letter bucket) or an
/// acquisition item (a book), distinguished only by which `links` it
/// carries.
#[derive(Debug, Clone, Default)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub updated: String,
    pub summary: Option<String>,
    pub authors: Vec<String>,
    pub links: Vec<Link>,
}

impl Entry {
    fn write(&self, out: &mut String) {
        out.push_str("<entry>");
        write_elem(out, "title", &self.title);
        write_elem(out, "id", &self.id);
        write_elem(out, "updated", &self.updated);
        for author in &self.authors {
            out.push_str("<author>");
            write_elem(out, "name", author);
            out.push_str("</author>");
        }
        if let Some(summary) = &self.summary {
            out.push_str("<summary type=\"text\">");
            out.push_str(&escape(summary));
            out.push_str("</summary>");
        }
        for link in &self.links {
            link.write(out);
        }
        out.push_str("</entry>");
    }
}

/// A complete OPDS Atom feed — either navigation (entries `<link
/// rel="subsection">` into other feeds) or acquisition (entries carry
/// `http://opds-spec.org/acquisition` download links). The two share one
/// wire shape; callers pick the feed-level [`NAVIGATION_TYPE`] /
/// [`ACQUISITION_TYPE`] content type to advertise which.
#[derive(Debug, Clone, Default)]
pub struct Feed {
    pub id: String,
    pub title: String,
    pub updated: String,
    pub links: Vec<Link>,
    pub entries: Vec<Entry>,
}

impl Feed {
    /// Serialize to a complete `<?xml ...?><feed>...</feed>` document.
    pub fn to_xml(&self) -> String {
        let mut out = String::with_capacity(512 + self.entries.len() * 256);
        out.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
        out.push_str(
            r#"<feed xmlns="http://www.w3.org/2005/Atom" xmlns:opds="http://opds-spec.org/2010/catalog">"#,
        );
        write_elem(&mut out, "id", &self.id);
        write_elem(&mut out, "title", &self.title);
        write_elem(&mut out, "updated", &self.updated);
        for link in &self.links {
            link.write(&mut out);
        }
        for entry in &self.entries {
            entry.write(&mut out);
        }
        out.push_str("</feed>");
        out
    }
}

/// Write a simple text element: `<name>escaped(text)</name>`.
fn write_elem(out: &mut String, name: &str, text: &str) {
    out.push('<');
    out.push_str(name);
    out.push('>');
    out.push_str(&escape(text));
    out.push_str("</");
    out.push_str(name);
    out.push('>');
}

/// Escape the five XML predefined entities. Safe for both text content and
/// (double-quoted) attribute values — used for both by this module.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_replaces_all_five_predefined_entities() {
        assert_eq!(
            escape(r#"Tom & Jerry's "Great" <Escape>"#),
            "Tom &amp; Jerry&apos;s &quot;Great&quot; &lt;Escape&gt;"
        );
    }

    #[test]
    fn escape_leaves_plain_text_untouched() {
        assert_eq!(escape("A Study in Scarlet"), "A Study in Scarlet");
    }

    #[test]
    fn feed_to_xml_emits_a_well_formed_document_with_escaped_content() {
        let feed = Feed {
            id: "urn:test".to_string(),
            title: "Rock & Roll".to_string(),
            updated: "2024-01-01T00:00:00Z".to_string(),
            links: vec![Link::new("self", "/opds", NAVIGATION_TYPE)],
            entries: vec![Entry {
                id: "urn:uuid:1".to_string(),
                title: "A & B".to_string(),
                updated: "2024-01-01T00:00:00Z".to_string(),
                summary: Some("<spoiler>".to_string()),
                authors: vec!["Jane \"J\" Doe".to_string()],
                links: vec![Link::new(
                    "http://opds-spec.org/acquisition",
                    "/api/ebooks/1/download",
                    "application/epub+zip",
                )],
            }],
        };
        let xml = feed.to_xml();
        assert!(xml.starts_with(r#"<?xml version="1.0" encoding="UTF-8"?>"#));
        assert!(xml.contains("<title>Rock &amp; Roll</title>"));
        assert!(xml.contains(r#"<link rel="self" href="/opds" type=""#));
        assert!(xml.contains("<title>A &amp; B</title>"));
        assert!(xml.contains("<summary type=\"text\">&lt;spoiler&gt;</summary>"));
        assert!(xml.contains("<name>Jane &quot;J&quot; Doe</name>"));
        assert!(xml.ends_with("</feed>"));
    }
}
