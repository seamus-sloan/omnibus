//! One candidate row, and the order the rows come in.

use dioxus::prelude::*;
use omnibus_shared::metadata_lookup::ProviderEdition;

use super::sources::provider_slug;

/// Shown for a field this candidate has no value for.
const EMPTY: &str = "\u{2014}";

/// Put candidates in an order derived from the editions themselves.
///
/// The fan-out answers provider by provider, so the raw list is "everything
/// Open Library found, then everything Google Books found" — which means a
/// provider being slow, newly configured, or briefly down reshuffles the
/// whole list under the reader. Sorting on the candidate's own fields makes
/// that impossible: a source dropping out removes its rows and moves nothing
/// else.
///
/// Title first, so a book's editions sit together and two sources' takes on
/// one printing land next to each other — the comparison the picker exists
/// for, without merging rows to get it. `(isbn13, source, provider_ref)`
/// finishes the key so no two candidates can tie and the order is total.
pub(super) fn in_stable_order(mut editions: Vec<ProviderEdition>) -> Vec<ProviderEdition> {
    editions.sort_by_cached_key(|e| {
        (
            e.title.trim().to_lowercase(),
            e.isbn13.clone(),
            provider_slug(e.source),
            e.provider_ref.clone(),
        )
    });
    editions
}

/// The row's second line: authors, or an em dash when the source named none.
fn authors_line(edition: &ProviderEdition) -> String {
    if edition.authors.is_empty() {
        EMPTY.to_string()
    } else {
        edition.authors.join(", ")
    }
}

/// The row's third line: year, publisher, ISBN — with the parts the provider
/// left empty dropped rather than rendered as gaps.
fn imprint_line(edition: &ProviderEdition) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(year) = edition.year.as_deref().filter(|v| !v.trim().is_empty()) {
        parts.push(year.to_string());
    }
    if let Some(publisher) = edition
        .publisher
        .as_deref()
        .filter(|v| !v.trim().is_empty())
    {
        parts.push(publisher.to_string());
    }
    parts.push(edition.isbn13.clone());
    parts.join(" \u{b7} ")
}

/// One candidate. Selecting it is the row's only action, so the whole row is
/// the control.
#[component]
pub(super) fn CandidateRow(
    index: usize,
    edition: ProviderEdition,
    on_select: EventHandler<ProviderEdition>,
) -> Element {
    let picked = edition.clone();
    // Explicit rather than inherited from the row's text: the accessible name
    // is otherwise the whole card, which reads as a paragraph and makes two
    // printings of one book indistinguishable by name.
    let label = format!(
        "Compare {} from {}",
        edition.title,
        edition.source.display_name()
    );
    rsx! {
        li { class: "mes-row-item",
            button {
                r#type: "button",
                class: "mes-row",
                "data-testid": "mes-candidate-{index}",
                aria_label: "{label}",
                onclick: move |_| on_select.call(picked.clone()),
                if let Some(url) = edition.cover_url.clone() {
                    img { class: "mes-row-cover", src: "{url}", alt: "", loading: "lazy" }
                } else {
                    span { class: "mes-row-cover mes-row-plate", aria_hidden: "true", "{EMPTY}" }
                }
                span { class: "mes-row-body",
                    span { class: "mes-row-title", "data-testid": "mes-candidate-{index}-title",
                        "{edition.title}"
                    }
                    span { class: "mes-row-authors", "data-testid": "mes-candidate-{index}-authors",
                        "{authors_line(&edition)}"
                    }
                    span { class: "mono mes-row-imprint", "data-testid": "mes-candidate-{index}-imprint",
                        "{imprint_line(&edition)}"
                    }
                }
                span {
                    class: "mes-badge",
                    "data-testid": "mes-candidate-{index}-source",
                    "data-source": "{provider_slug(edition.source)}",
                    "{edition.source.display_name()}"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use omnibus_shared::metadata_lookup::MetadataProvider;

    use super::*;

    fn edition(title: &str, isbn13: &str, source: MetadataProvider) -> ProviderEdition {
        ProviderEdition {
            source,
            provider_ref: format!("{source:?}-{isbn13}"),
            isbn13: isbn13.into(),
            isbn10: None,
            title: title.into(),
            authors: vec!["Joshua Bloch".into()],
            year: Some("2018".into()),
            pages: Some(416),
            publisher: Some("Addison-Wesley".into()),
            description: None,
            cover_url: None,
            series: None,
            first_publish_year: None,
            genres: Vec::new(),
        }
    }

    #[test]
    fn in_stable_order_is_independent_of_the_order_providers_answered_in() {
        // The property that matters: the same set sorts the same way however
        // the fan-out happened to concatenate it.
        let a = edition(
            "Effective Java",
            "9780134685991",
            MetadataProvider::OpenLibrary,
        );
        let b = edition(
            "Effective Java",
            "9780134685991",
            MetadataProvider::GoogleBooks,
        );
        let c = edition("Dune", "9780441013593", MetadataProvider::Hardcover);

        let one = in_stable_order(vec![a.clone(), b.clone(), c.clone()]);
        let two = in_stable_order(vec![c.clone(), b.clone(), a.clone()]);
        let three = in_stable_order(vec![b, a, c]);
        assert_eq!(one, two);
        assert_eq!(two, three);
    }

    #[test]
    fn in_stable_order_leaves_the_rest_in_place_when_a_provider_drops_out() {
        // A source going down must remove its rows and move nothing else —
        // the whole reason the key comes from the edition rather than from
        // who answered.
        let ol = edition(
            "Effective Java",
            "9780134685991",
            MetadataProvider::OpenLibrary,
        );
        let gb = edition("Dune", "9780441013593", MetadataProvider::GoogleBooks);
        let hc = edition("Neuromancer", "9780441569595", MetadataProvider::Hardcover);

        let full = in_stable_order(vec![ol.clone(), gb.clone(), hc.clone()]);
        let without_gb = in_stable_order(vec![ol, hc]);
        let expected: Vec<_> = full.into_iter().filter(|e| e.title != "Dune").collect();
        assert_eq!(without_gb, expected);
    }

    #[test]
    fn in_stable_order_groups_one_titles_editions_together() {
        let mixed = in_stable_order(vec![
            edition("Dune", "9780441013593", MetadataProvider::GoogleBooks),
            edition(
                "Effective Java",
                "9780134685991",
                MetadataProvider::OpenLibrary,
            ),
            edition("Dune", "9780441172719", MetadataProvider::OpenLibrary),
        ]);
        let titles: Vec<&str> = mixed.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, vec!["Dune", "Dune", "Effective Java"]);
    }

    #[test]
    fn in_stable_order_is_case_insensitive_on_the_title() {
        let sorted = in_stable_order(vec![
            edition("dune", "9780441013593", MetadataProvider::GoogleBooks),
            edition("Alpha", "9780441172719", MetadataProvider::OpenLibrary),
        ]);
        assert_eq!(sorted[0].title, "Alpha");
    }

    #[test]
    fn authors_line_falls_back_to_an_em_dash_when_the_provider_named_none() {
        let mut e = edition(
            "Effective Java",
            "9780134685991",
            MetadataProvider::OpenLibrary,
        );
        e.authors.clear();
        assert_eq!(authors_line(&e), EMPTY);
    }

    #[test]
    fn imprint_line_drops_the_parts_the_provider_left_empty() {
        let mut e = edition(
            "Effective Java",
            "9780134685991",
            MetadataProvider::OpenLibrary,
        );
        e.year = None;
        e.publisher = Some("   ".into());
        // The ISBN is never dropped — it is the one field every candidate is
        // required to carry.
        assert_eq!(imprint_line(&e), "9780134685991");
    }

    #[test]
    fn imprint_line_joins_year_publisher_and_isbn_when_all_are_present() {
        assert_eq!(
            imprint_line(&edition(
                "Effective Java",
                "9780134685991",
                MetadataProvider::OpenLibrary
            )),
            "2018 \u{b7} Addison-Wesley \u{b7} 9780134685991"
        );
    }
}
