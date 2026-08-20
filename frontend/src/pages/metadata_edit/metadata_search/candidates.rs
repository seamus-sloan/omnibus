//! The candidate list: one row per edition a provider returned.
//!
//! A search commonly answers with several printings of one book across
//! several sources, so a row carries exactly what tells two of them apart at
//! a glance — cover, title, authors, year, publisher, and which source it
//! came from — and nothing more. Provider cover URLs render straight from
//! the provider; nothing is stored until something is applied.

use dioxus::prelude::*;
use omnibus_shared::metadata_lookup::ProviderEdition;

use super::sources::provider_slug;

/// An em dash for a field this candidate has no value for, so the row's
/// shape stays the same whether or not a provider filled it in.
const EMPTY: &str = "\u{2014}";

/// Human summary of one candidate's authors, or the em dash.
fn authors_line(edition: &ProviderEdition) -> String {
    if edition.authors.is_empty() {
        EMPTY.to_string()
    } else {
        edition.authors.join(", ")
    }
}

/// The row's second line: year, publisher, and ISBN-13, in one string with
/// the parts the provider left empty dropped rather than rendered as gaps.
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

/// The list of candidates, or the empty-result note when every source came
/// back with nothing.
#[component]
pub(super) fn CandidateList(
    editions: Vec<ProviderEdition>,
    on_select: EventHandler<ProviderEdition>,
) -> Element {
    if editions.is_empty() {
        return rsx! {
            p { class: "mes-note", role: "status", "data-testid": "mes-empty",
                "No editions matched. Try a shorter title, or add the author."
            }
        };
    }
    rsx! {
        ul { class: "mes-candidates", "data-testid": "mes-candidates",
            for (index , edition) in editions.into_iter().enumerate() {
                CandidateRow { index, edition, on_select }
            }
        }
    }
}

/// One candidate. The whole row is the control — selecting it is the only
/// action a row has, so a button wrapping the content beats a button beside
/// it.
#[component]
fn CandidateRow(
    index: usize,
    edition: ProviderEdition,
    on_select: EventHandler<ProviderEdition>,
) -> Element {
    let picked = edition.clone();
    // Explicit rather than inherited from the row's text: the accessible
    // name is otherwise the whole card, which reads as a paragraph and makes
    // two printings of one book indistinguishable by name.
    let label = format!(
        "Compare {} from {}",
        edition.title,
        edition.source.display_name()
    );
    rsx! {
        li { class: "mes-candidate-item",
            button {
                r#type: "button",
                class: "mes-candidate",
                "data-testid": "mes-candidate-{index}",
                aria_label: "{label}",
                onclick: move |_| on_select.call(picked.clone()),
                div { class: "mes-candidate-cover",
                    if let Some(url) = edition.cover_url.clone() {
                        img {
                            class: "mes-candidate-img",
                            src: "{url}",
                            alt: "",
                            loading: "lazy",
                        }
                    } else {
                        span { class: "mes-candidate-plate", aria_hidden: "true", "{EMPTY}" }
                    }
                }
                div { class: "mes-candidate-body",
                    div { class: "mes-candidate-title", "data-testid": "mes-candidate-{index}-title",
                        "{edition.title}"
                    }
                    div { class: "mes-candidate-authors", "data-testid": "mes-candidate-{index}-authors",
                        "{authors_line(&edition)}"
                    }
                    div { class: "mono mes-candidate-imprint", "data-testid": "mes-candidate-{index}-imprint",
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

    fn edition() -> ProviderEdition {
        ProviderEdition {
            source: MetadataProvider::OpenLibrary,
            provider_ref: "/works/OL1W".into(),
            isbn13: "9780134685991".into(),
            isbn10: None,
            title: "Effective Java".into(),
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
    fn authors_line_falls_back_to_an_em_dash_when_the_provider_named_none() {
        let mut e = edition();
        e.authors.clear();
        assert_eq!(authors_line(&e), EMPTY);
    }

    #[test]
    fn imprint_line_drops_the_parts_the_provider_left_empty() {
        let mut e = edition();
        e.year = None;
        e.publisher = Some("   ".into());
        // Only the ISBN survives — and it is never dropped, since it is the
        // one field every candidate is required to carry.
        assert_eq!(imprint_line(&e), "9780134685991");
    }

    #[test]
    fn imprint_line_joins_year_publisher_and_isbn_when_all_are_present() {
        assert_eq!(
            imprint_line(&edition()),
            "2018 \u{b7} Addison-Wesley \u{b7} 9780134685991"
        );
    }
}
