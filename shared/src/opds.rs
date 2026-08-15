//! OPDS 2.0 (Readium Web Publication Manifest) wire types, serialized as
//! `application/opds+json` by the `json_*` handlers under
//! `server/src/backend/opds/` (`json_nav.rs`, `json_search.rs`, etc). A parallel
//! presentation of the same navigation/acquisition data the OPDS 1.2 Atom
//! catalog at `/opds/*` exposes (hand-rolled XML in
//! `server/src/backend/opds/atom.rs`) for OPDS clients that speak JSON
//! instead. Unlike Atom, one media type covers both navigation and
//! publication feeds — the populated array (`navigation` vs `publications`)
//! already tells a client which kind it got.
//!
//! Deliberately *not* re-exported flat from the crate root — `Contributor`
//! collides with `ebook::Contributor` — so callers reach these types via
//! `omnibus_shared::opds::*`, matching the `http_range` precedent.

use serde::{Deserialize, Serialize};

/// MIME type for an OPDS 2.0 JSON catalog document (feed or publication
/// list) — the JSON counterpart to Atom's `NAVIGATION_TYPE` /
/// `ACQUISITION_TYPE` pair.
pub const MEDIA_TYPE: &str = "application/opds+json";

/// A Readium Web Publication Manifest Link Object. Used both for feed-level
/// navigation (`rel="subsection"` into another feed) and for a
/// publication's acquisition/cover links — the two are distinguished only
/// by `rel`, mirroring Atom's `Link`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Link {
    pub href: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// RFC 6570 URI Template flag (e.g. `/opds/v2/search{?q}`). Omitted
    /// (defaults `false`) rather than serialized when unset, since most
    /// links are plain hrefs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub templated: bool,
}

impl Link {
    /// Build a plain link with just an `href`; chain the `with_*` builders
    /// for the optional fields actually needed at each call site.
    pub fn new(href: impl Into<String>) -> Self {
        Self {
            href: href.into(),
            ..Default::default()
        }
    }

    pub fn with_rel(mut self, rel: impl Into<String>) -> Self {
        self.rel = Some(rel.into());
        self
    }

    pub fn with_type(mut self, type_: impl Into<String>) -> Self {
        self.type_ = Some(type_.into());
        self
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn templated(mut self) -> Self {
        self.templated = true;
        self
    }
}

/// A publication's author/contributor — just a display name. The Readium
/// manifest shape allows a bare string or an object; Omnibus always emits
/// the object form for consistency.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Contributor {
    pub name: String,
}

/// One subject/category term (`EbookMetadata::subjects` or `::genres`).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Subject {
    pub name: String,
}

/// One series a publication belongs to, with its optional index within it.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SeriesRef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<f64>,
}

/// The `belongsTo` collection-membership block. Only `series` is populated
/// today — Omnibus has no other collection concept to report here.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct BelongsTo {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub series: Vec<SeriesRef>,
}

/// A publication's `metadata` object — the Readium/schema.org-flavored
/// fields an OPDS 2.0 client reads to render a catalog entry without
/// following any link.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PublicationMetadata {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub schema_type: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub author: Vec<Contributor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subject: Vec<Subject>,
    #[serde(rename = "belongsTo", default, skip_serializing_if = "Option::is_none")]
    pub belongs_to: Option<BelongsTo>,
}

/// One publication (book) entry — the JSON counterpart to an Atom
/// acquisition `<entry>`. `links` carries the acquisition/cover/thumbnail
/// links (Atom's `Entry::links`); `images` is the Readium-manifest-standard
/// home for cover art specifically.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Publication {
    pub metadata: PublicationMetadata,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Link>,
}

/// A feed's `metadata` object.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct FeedMetadata {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(
        rename = "numberOfItems",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub number_of_items: Option<i64>,
}

/// A complete OPDS 2.0 catalog document — either a navigation feed
/// (`navigation` populated, `publications` empty) or a publication feed
/// (the reverse). Both shapes share this one envelope, matching the single
/// [`MEDIA_TYPE`] both are served as.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Feed {
    pub metadata: FeedMetadata,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub navigation: Vec<Link>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub publications: Vec<Publication>,
}

#[cfg(test)]
mod tests;
