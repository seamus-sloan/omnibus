//! `/opds` root navigation feed and `/opds/osd` OpenSearch description
//! (AC1, AC3).

use axum::response::Response;

use super::atom::{Feed, Link, ACQUISITION_TYPE, NAVIGATION_TYPE, OPENSEARCH_TYPE};
use super::{nav_entry, now_rfc3339, xml_response};
use crate::auth::AuthUser;

/// `GET /opds` — the root navigation feed. Links to search (`rel="search"`)
/// and entries into recently-added and the author browse (AC1). Static: no
/// DB read, since it only advertises the other endpoints' existence.
pub(super) async fn root(_user: AuthUser) -> Response {
    let updated = now_rfc3339();
    let feed = Feed {
        id: "urn:omnibus:opds:root".to_string(),
        title: "Omnibus".to_string(),
        updated: updated.clone(),
        links: vec![
            Link::new("self", "/opds", NAVIGATION_TYPE),
            Link::new("start", "/opds", NAVIGATION_TYPE),
            Link::new("search", "/opds/osd", OPENSEARCH_TYPE),
        ],
        entries: vec![
            nav_entry(
                "urn:omnibus:opds:new",
                "Recently Added",
                &updated,
                "/opds/new",
                ACQUISITION_TYPE,
                "The newest books in the library",
            ),
            nav_entry(
                "urn:omnibus:opds:authors",
                "Authors",
                &updated,
                "/opds/authors",
                NAVIGATION_TYPE,
                "Browse by author, A to Z",
            ),
        ],
    };
    xml_response(NAVIGATION_TYPE, feed.to_xml())
}

/// The static OpenSearch description body `/opds/osd` serves (AC3). Its one
/// `Url` template points `/opds/search?q={searchTerms}` back at an
/// acquisition feed; there is nothing per-request to fill in, so this is a
/// constant rather than a builder.
const OSD_XML: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8"?>"#,
    r#"<OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/">"#,
    "<ShortName>Omnibus</ShortName>",
    "<Description>Search the Omnibus library</Description>",
    "<InputEncoding>UTF-8</InputEncoding>",
    "<OutputEncoding>UTF-8</OutputEncoding>",
    r#"<Url type="application/atom+xml;profile=opds-catalog;kind=acquisition" template="/opds/search?q={searchTerms}"/>"#,
    "</OpenSearchDescription>"
);

/// `GET /opds/osd` — the OpenSearch description document `rel="search"` on
/// the root feed points at (AC3).
pub(super) async fn osd(_user: AuthUser) -> Response {
    xml_response(OPENSEARCH_TYPE, OSD_XML.to_string())
}
