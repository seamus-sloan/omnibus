use super::*;

#[test]
fn link_new_serializes_only_the_href_when_no_builder_is_chained() {
    let link = Link::new("/opds/v2");
    let json = serde_json::to_value(&link).unwrap();
    assert_eq!(json, serde_json::json!({ "href": "/opds/v2" }));
}

#[test]
fn link_builders_populate_type_rel_title_and_templated() {
    let link = Link::new("/opds/v2/search{?q}")
        .with_rel("search")
        .with_type(MEDIA_TYPE)
        .with_title("Search")
        .templated();
    let json = serde_json::to_value(&link).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "href": "/opds/v2/search{?q}",
            "type": MEDIA_TYPE,
            "rel": "search",
            "title": "Search",
            "templated": true,
        })
    );
}

#[test]
fn publication_metadata_omits_empty_optional_fields() {
    let metadata = PublicationMetadata {
        title: "Dune".to_string(),
        ..Default::default()
    };
    let json = serde_json::to_value(&metadata).unwrap();
    assert_eq!(json, serde_json::json!({ "title": "Dune" }));
}

#[test]
fn publication_metadata_serializes_series_and_subjects_when_present() {
    let metadata = PublicationMetadata {
        title: "Dune".to_string(),
        author: vec![Contributor {
            name: "Frank Herbert".to_string(),
        }],
        subject: vec![Subject {
            name: "Science Fiction".to_string(),
        }],
        belongs_to: Some(BelongsTo {
            series: vec![SeriesRef {
                name: "Dune".to_string(),
                position: Some(1.0),
            }],
        }),
        ..Default::default()
    };
    let json = serde_json::to_value(&metadata).unwrap();
    assert_eq!(json["author"][0]["name"], "Frank Herbert");
    assert_eq!(json["subject"][0]["name"], "Science Fiction");
    assert_eq!(json["belongsTo"]["series"][0]["name"], "Dune");
    assert_eq!(json["belongsTo"]["series"][0]["position"], 1.0);
}

#[test]
fn feed_serializes_navigation_and_omits_empty_publications() {
    let feed = Feed {
        metadata: FeedMetadata {
            title: "Omnibus".to_string(),
            ..Default::default()
        },
        links: vec![Link::new("/opds/v2").with_rel("self")],
        navigation: vec![Link::new("/opds/v2/new")
            .with_rel("subsection")
            .with_title("Recently Added")],
        publications: Vec::new(),
    };
    let json = serde_json::to_value(&feed).unwrap();
    assert!(json.get("publications").is_none());
    assert_eq!(json["navigation"][0]["title"], "Recently Added");
    assert_eq!(json["metadata"]["title"], "Omnibus");
}

#[test]
fn feed_round_trips_through_json() {
    let feed = Feed {
        metadata: FeedMetadata {
            title: "Search: dune".to_string(),
            modified: Some("2024-01-01T00:00:00Z".to_string()),
            number_of_items: Some(1),
        },
        links: vec![Link::new("/opds/v2/search?q=dune").with_rel("self")],
        navigation: Vec::new(),
        publications: vec![Publication {
            metadata: PublicationMetadata {
                title: "Dune".to_string(),
                identifier: Some("urn:uuid:abc".to_string()),
                ..Default::default()
            },
            links: vec![Link::new("/api/ebooks/abc/download")
                .with_rel("http://opds-spec.org/acquisition")
                .with_type("application/epub+zip")],
            images: vec![Link::new("/api/covers/abc").with_type("image/jpeg")],
        }],
    };
    let json = serde_json::to_string(&feed).unwrap();
    let round_tripped: Feed = serde_json::from_str(&json).unwrap();
    assert_eq!(round_tripped, feed);
}
