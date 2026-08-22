//! One candidate row, and the order the rows come in.

use dioxus::prelude::*;
use omnibus_shared::metadata_lookup::ProviderEdition;

use super::sources::provider_slug;
use super::EMPTY;

/// Put candidates in an order derived from the candidates themselves.
///
/// Two problems, one key. The fan-out answers provider by provider, so the
/// raw list is "everything Open Library found, then everything Google Books
/// found" — a source being slow, newly configured, or briefly down reshuffles
/// the whole list under the reader. And within a provider the order is
/// whatever it felt like, which for a well-known title is reliably four study
/// guides above the novel.
///
/// So: candidates whose title accounts for every word of the query first,
/// shortest title first within that, then the edition's own fields to finish
/// the key. Nothing here reads *who* answered until the final tiebreak, so a
/// provider dropping out removes its rows and moves nothing else — and
/// nothing is merged or discarded.
pub(super) fn in_stable_order(
    mut editions: Vec<ProviderEdition>,
    query: &str,
) -> Vec<ProviderEdition> {
    let words = query_words(query);
    editions.sort_by_cached_key(|e| {
        let title = e.title.trim().to_lowercase();
        (
            // `false` sorts first, so this reads "not a full match" — the
            // rows that account for the whole query lead.
            !covers_every_word(&haystack(e), &words),
            // A title that *is* the book beats one that wraps it in
            // "A Study Guide for …".
            title.chars().count(),
            title.clone(),
            e.isbn13.clone(),
            provider_slug(e.source),
            e.provider_ref.clone(),
        )
    });
    editions
}

/// The text a query is matched against: title **and** authors.
///
/// The seeded query is "title + primary author", so matching the title alone
/// would rank every candidate that merely names the author in its title —
/// a study guide — above the book itself, which of course doesn't repeat its
/// own author in its title.
fn haystack(edition: &ProviderEdition) -> String {
    format!("{} {}", edition.title, edition.authors.join(" ")).to_lowercase()
}

/// Words that appear in so many titles that requiring them ranks nothing.
/// Listed rather than inferred from length: "The" and "Ida" are both three
/// letters and only one of them is noise.
const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "by", "for", "from", "in", "of", "on", "or", "the", "to", "with",
];

/// The query's words, lowercased and stripped of punctuation, minus the stop
/// words and the initials a name leaves behind.
fn query_words(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| w.chars().count() > 1 && !STOP_WORDS.contains(&w.as_str()))
        .collect()
}

/// Whether `haystack` accounts for every word in `words`. An empty word list
/// matches nothing, so a blank query leaves the order to the later keys
/// rather than declaring every row a perfect match.
fn covers_every_word(haystack: &str, words: &[String]) -> bool {
    !words.is_empty() && words.iter().all(|w| haystack.contains(w.as_str()))
}

/// The candidate's cover URL, or `None` when the provider gave nothing
/// usable. Blank-but-present is the case worth catching: rendered as an
/// `<img src>` it resolves against the current page and pulls the HTML
/// document down as an image.
fn cover_src(edition: &ProviderEdition) -> Option<String> {
    edition
        .cover_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_string)
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
                // Trimmed, not just `Some`: providers don't consistently trim
                // this, and `src="   "` resolves against the current page —
                // a request that fetches the HTML document as an image.
                if let Some(url) = cover_src(&edition) {
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
            series_index: None,
            first_publish_year: None,
            genres: Vec::new(),
        }
    }

    /// Every test's query, unless it says otherwise.
    const Q: &str = "Effective Java";

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

        let one = in_stable_order(vec![a.clone(), b.clone(), c.clone()], Q);
        let two = in_stable_order(vec![c.clone(), b.clone(), a.clone()], Q);
        let three = in_stable_order(vec![b, a, c], Q);
        assert_eq!(one, two);
        assert_eq!(two, three);
    }

    #[test]
    fn in_stable_order_leaves_the_rest_in_place_when_a_provider_drops_out() {
        // A source going down must remove its rows and move nothing else —
        // the whole reason no key but the last tiebreak reads who answered.
        let ol = edition(
            "Effective Java",
            "9780134685991",
            MetadataProvider::OpenLibrary,
        );
        let gb = edition("Dune", "9780441013593", MetadataProvider::GoogleBooks);
        let hc = edition("Neuromancer", "9780441569595", MetadataProvider::Hardcover);

        let full = in_stable_order(vec![ol.clone(), gb.clone(), hc.clone()], Q);
        let without_gb = in_stable_order(vec![ol, hc], Q);
        let expected: Vec<_> = full.into_iter().filter(|e| e.title != "Dune").collect();
        assert_eq!(without_gb, expected);
    }

    /// Like [`edition`], but with the authors a real provider would return —
    /// what the query's author half is matched against.
    fn by(title: &str, isbn13: &str, author: &str) -> ProviderEdition {
        ProviderEdition {
            authors: vec![author.into()],
            ..edition(title, isbn13, MetadataProvider::OpenLibrary)
        }
    }

    #[test]
    fn in_stable_order_leads_with_the_titles_that_account_for_the_whole_query() {
        // The observed failure this key exists for: a well-known title comes
        // back with four study guides above the book itself.
        let sorted = in_stable_order(
            vec![
                by(
                    "A Study Guide for Ursula K. LeGuin's The Left Hand of Darkness",
                    "9781410336088",
                    "Gale, Cengage Learning",
                ),
                by(
                    "Gender under the Viking Age",
                    "9798745605864",
                    "Rusty Brient",
                ),
                by(
                    "The Left Hand of Darkness",
                    "9780441478125",
                    "Ursula K. Le Guin",
                ),
            ],
            "The Left Hand of Darkness Ursula K. Le Guin",
        );
        let titles: Vec<&str> = sorted.iter().map(|e| e.title.as_str()).collect();
        // The novel first — it accounts for every word and is the shortest
        // title that does. The study guide accounts for them too but says
        // more; the unrelated book accounts for none.
        assert_eq!(titles[0], "The Left Hand of Darkness");
        assert_eq!(titles[2], "Gender under the Viking Age");
    }

    #[test]
    fn in_stable_order_falls_back_to_the_editions_own_fields_for_a_blank_query() {
        // No query words means nothing is a match, so the order must come
        // from the later keys rather than from every row tying at the top.
        let sorted = in_stable_order(
            vec![
                edition("Zebra", "9780441013593", MetadataProvider::GoogleBooks),
                edition("Alpha", "9780441172719", MetadataProvider::OpenLibrary),
            ],
            "   ",
        );
        assert_eq!(sorted[0].title, "Alpha");
    }

    #[test]
    fn in_stable_order_groups_one_titles_editions_together() {
        let mixed = in_stable_order(
            vec![
                edition("Dune", "9780441013593", MetadataProvider::GoogleBooks),
                edition(
                    "Dune Messiah",
                    "9780441172696",
                    MetadataProvider::OpenLibrary,
                ),
                edition("Dune", "9780441172719", MetadataProvider::OpenLibrary),
            ],
            "Dune",
        );
        let titles: Vec<&str> = mixed.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, vec!["Dune", "Dune", "Dune Messiah"]);
    }

    #[test]
    fn query_words_drops_the_connectives_that_match_everything() {
        assert_eq!(
            query_words("The Left Hand of Darkness"),
            vec!["left", "hand", "darkness"]
        );
    }

    #[test]
    fn query_words_drops_a_names_initials_but_keeps_its_words() {
        // "K." carries no signal and would fail every haystack that spells
        // the name differently.
        assert_eq!(
            query_words("Ursula K. Le Guin"),
            vec!["ursula", "le", "guin"]
        );
    }

    #[test]
    fn haystack_covers_the_authors_as_well_as_the_title() {
        // The seeded query is title + author, and a book does not repeat its
        // own author in its title.
        let e = by(
            "The Left Hand of Darkness",
            "9780441478125",
            "Ursula K. Le Guin",
        );
        assert!(covers_every_word(
            &haystack(&e),
            &query_words("The Left Hand of Darkness Ursula K. Le Guin")
        ));
    }

    #[test]
    fn cover_src_treats_a_blank_provider_url_as_absent() {
        // `src="   "` resolves against the current page, so the row would
        // fetch the HTML document and render it as a broken image.
        let mut e = edition(
            "Effective Java",
            "9780134685991",
            MetadataProvider::OpenLibrary,
        );
        e.cover_url = Some("   ".into());
        assert_eq!(cover_src(&e), None);
        e.cover_url = None;
        assert_eq!(cover_src(&e), None);
        e.cover_url = Some("  https://covers.example/1.jpg  ".into());
        assert_eq!(
            cover_src(&e).as_deref(),
            Some("https://covers.example/1.jpg")
        );
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
