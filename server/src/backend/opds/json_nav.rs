//! `/opds/v2` root navigation feed (AC1) — the OPDS 2.0 JSON counterpart to
//! `/opds`. No OpenSearch description document: OPDS 2.0's own answer to
//! "how do I search" is a `rel="search"` `Link` with `templated: true`
//! (RFC 6570), so this feed carries the template directly rather than
//! pointing at a second XML document the way `/opds/osd` does.

use axum::response::Response;
use omnibus_shared::opds::{Feed, FeedMetadata, Link, MEDIA_TYPE};

use super::{json_nav_link, json_response};
use crate::auth::OpdsAuthUser;

/// `GET /opds/v2` — the root navigation feed. Links to search
/// (`rel="search"`, templated) and navigation entries into recently-added,
/// the author browse, and the series browse (AC1). Static: no DB read,
/// since it only advertises the other endpoints' existence.
pub(super) async fn root(_user: OpdsAuthUser) -> Response {
    let feed = Feed {
        metadata: FeedMetadata {
            title: "Omnibus".to_string(),
            ..Default::default()
        },
        links: vec![
            Link::new("/opds/v2").with_rel("self").with_type(MEDIA_TYPE),
            Link::new("/opds/v2")
                .with_rel("start")
                .with_type(MEDIA_TYPE),
            Link::new("/opds/v2/search{?q}")
                .with_rel("search")
                .with_type(MEDIA_TYPE)
                .templated(),
        ],
        navigation: vec![
            json_nav_link("All Books", "/opds/v2/all"),
            json_nav_link("Recently Added", "/opds/v2/new"),
            json_nav_link("Authors", "/opds/v2/authors"),
            json_nav_link("Series", "/opds/v2/series"),
        ],
        publications: Vec::new(),
    };
    json_response(&feed)
}
