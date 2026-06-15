//! Authors index page. Lists every author in the library as a browsable
//! surface, mirroring the `AuthorsIndex` design comp from
//! `screens/indices.jsx`.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::AuthorSummary;

use crate::{data, use_server_url, Route};

/// Sort axes exposed in the toolbar. Mirrors the design's "Last name A–Z /
/// Most books" toggle. "Recently added" and "Highest rated" from the comp
/// require fields the v1 summary doesn't carry — deferred to a follow-up
/// once `AuthorSummary` grows.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Sort {
    Name,
    BookCount,
}

/// Library-wide totals rendered in the header's subtitle. Grouped so the
/// header doesn't grow a long `total_*` prop list.
#[derive(Clone, PartialEq, Eq)]
struct AuthorIndexCounts {
    total_authors: usize,
    total_books: usize,
}

/// Filter + sort + alphabet-strip state for the header. The strip's
/// visibility, glyphs, and footnote totals all change together as the
/// filtered set changes, so they ride the same struct.
#[derive(Clone, PartialEq, Eq)]
struct AuthorIndexFilter {
    filter: String,
    sort: Sort,
    show_letters: bool,
    alphabet: Vec<char>,
    present_letters: std::collections::HashSet<char>,
    letter_count: usize,
    filtered_count: usize,
}

/// Authors index page — browse all authors in the library.
#[component]
pub fn AuthorsIndexPage() -> Element {
    let server_url = use_server_url();
    let server_url_for_cards = server_url.clone();
    let mut authors: Signal<Vec<AuthorSummary>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut filter = use_signal(String::new);
    let mut sort = use_signal(|| Sort::Name);

    let url = server_url.clone();
    use_effect(move || {
        let url = url.clone();
        spawn(async move {
            loading.set(true);
            match data::list_authors(&url).await {
                Ok(a) => {
                    authors.set(a);
                    error.set(None);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            loading.set(false);
        });
    });

    if loading() {
        return rsx! {
            p { class: "subtitle", "Loading\u{2026}" }
        };
    }
    if let Some(msg) = error() {
        return rsx! {
            p { role: "alert", class: "subtitle", "{msg}" }
            Link { to: Route::Landing {}, class: "btn", "Back to library" }
        };
    }

    // Borrow the signal's contents instead of cloning. The list can hold
    // up to `INDEX_LIMIT` (10k) rows; copying the whole `Vec` on every
    // re-render (which fires on each keystroke in the filter input) would
    // be wasted work. `Signal::read` hands back a guard that derefs to
    // `&Vec<AuthorSummary>` and the rsx! tree only needs borrowed views.
    let all = authors.read();
    let total_authors = all.len();
    let total_books: usize = all.iter().map(|a| a.book_count).sum();

    // Client-side filter + sort.
    let q = filter.read().to_lowercase();
    let mut filtered: Vec<&AuthorSummary> = if q.is_empty() {
        all.iter().collect()
    } else {
        all.iter()
            .filter(|a| a.name.to_lowercase().contains(&q))
            .collect()
    };
    match sort() {
        // Within Sort::Name the primary axis is the sort_key bucket:
        // alpha first (`is_alpha_bucket` → false sorts before true),
        // non-alpha (digits, punctuation, accented mononyms, etc.)
        // trail at the end under the '#' section. Secondary axis is
        // the lowercased key itself so cards stay alphabetical within
        // their letter group.
        Sort::Name => filtered.sort_by(|a, b| {
            let ka = sort_key(a);
            let kb = sort_key(b);
            is_non_alpha_key(&ka)
                .cmp(&is_non_alpha_key(&kb))
                .then_with(|| ka.to_lowercase().cmp(&kb.to_lowercase()))
        }),
        Sort::BookCount => filtered.sort_by(|a, b| {
            b.book_count
                .cmp(&a.book_count)
                .then_with(|| sort_key(a).to_lowercase().cmp(&sort_key(b).to_lowercase()))
        }),
    }

    // Group by first letter of sort key (last name when available, else name).
    // Only meaningful when sorting by name.
    let show_letters = matches!(sort(), Sort::Name);
    let mut letters: Vec<(char, Vec<&AuthorSummary>)> = Vec::new();
    if show_letters {
        for a in &filtered {
            let l = first_letter(a);
            match letters.last_mut() {
                Some((existing, group)) if *existing == l => group.push(a),
                _ => letters.push((l, vec![a])),
            }
        }
    }
    // Alphabet strip: A–Z followed by a single '#' bucket for any
    // authors whose surname-equivalent doesn't start with an ASCII
    // letter (digits, punctuation, accented mononyms, …). The '#'
    // glyph is rendered after Z so the eye lands on the alpha range
    // first and the long tail of edge cases sits where it expects to.
    let alphabet: Vec<char> = ('A'..='Z').chain(std::iter::once('#')).collect();
    let present_letters: std::collections::HashSet<char> =
        letters.iter().map(|(l, _)| *l).collect();

    let any_in_library = !all.is_empty();

    let counts = AuthorIndexCounts {
        total_authors,
        total_books,
    };
    let filter_state = AuthorIndexFilter {
        filter: filter(),
        sort: sort(),
        show_letters,
        alphabet: alphabet.clone(),
        present_letters: present_letters.clone(),
        letter_count: letters.len(),
        filtered_count: filtered.len(),
    };

    rsx! {
        div { class: "idx-page",
            AuthorsIndexHeader {
                counts,
                filter_state,
                on_filter: move |v| filter.set(v),
                on_sort: move |s| sort.set(s),
            }
            {authors_index_body(&filtered, &letters, show_letters, any_in_library, &server_url_for_cards)}
        }
    }
}

/// Body section (plain fn, not `#[component]`, so we can keep the borrow-only
/// slices from `AuthorsIndexPage`'s signal read guard without per-render clones).
fn authors_index_body<'a>(
    filtered: &[&'a AuthorSummary],
    letters: &[(char, Vec<&'a AuthorSummary>)],
    show_letters: bool,
    any_in_library: bool,
    server_url: &str,
) -> Element {
    rsx! {
        div { class: "idx-body",
            if filtered.is_empty() {
                p { class: "subtitle idx-empty",
                    if !any_in_library {
                        "No authors yet \u{2014} the library is empty."
                    } else {
                        "No authors match that filter."
                    }
                }
            } else if show_letters {
                for (letter, group) in letters.iter() {
                    section {
                        key: "{letter}",
                        id: "letter-{letter_frag(*letter)}",
                        class: "idx-letter-section",
                        div { class: "idx-letter-rail",
                            div { class: "idx-letter-rail-glyph", "{letter}" }
                            div { class: "idx-letter-rail-count mono", "{group.len()}" }
                        }
                        div { class: "idx-card-grid",
                            for a in group.iter() {
                                div { key: "{a.id}", {render_author_card(a, server_url)} }
                            }
                        }
                    }
                }
            } else {
                div { class: "idx-card-grid idx-card-grid--flat",
                    for a in filtered.iter() {
                        div { key: "{a.id}", {render_author_card(a, server_url)} }
                    }
                }
            }
        }
    }
}

/// Header: breadcrumb, hero, filter/sort toolbar, optional A–Z letter strip.
#[component]
fn AuthorsIndexHeader(
    counts: AuthorIndexCounts,
    filter_state: AuthorIndexFilter,
    on_filter: EventHandler<String>,
    on_sort: EventHandler<Sort>,
) -> Element {
    let AuthorIndexCounts {
        total_authors,
        total_books,
    } = counts;
    let AuthorIndexFilter {
        filter,
        sort,
        show_letters,
        alphabet,
        present_letters,
        letter_count,
        filtered_count,
    } = filter_state;
    rsx! {
        div { class: "idx-header",
            nav {
                class: "breadcrumb",
                aria_label: "breadcrumb",
                Link { to: Route::Landing {}, "Library" }
                span { class: "breadcrumb-sep", " › " }
                span { "Authors" }
            }
            div { class: "idx-head-row",
                div {
                    span { class: "label", "Library lens" }
                    h1 { class: "disc-hero-title",
                        "By "
                        em { "author" }
                        "."
                    }
                    p { class: "idx-subtitle",
                        "{total_authors} authors across {total_books} books \u{b7} the people in your shelves."
                    }
                }
            }
            div { class: "idx-toolbar",
                FilterInput { filter, on_filter }
                SortSelector { sort, on_sort }
            }
            if show_letters {
                LetterNav {
                    alphabet: alphabet.clone(),
                    present_letters: present_letters.clone(),
                    letter_count,
                    filtered_count,
                }
            }
        }
    }
}

/// Search input that filters the author list by display name as the admin types.
#[component]
fn FilterInput(filter: String, on_filter: EventHandler<String>) -> Element {
    rsx! {
        div { class: "idx-search",
            input {
                r#type: "search",
                placeholder: "Filter authors by name\u{2026}",
                aria_label: "Filter authors by name",
                value: "{filter}",
                "data-testid": "authors-filter",
                oninput: move |e| on_filter.call(e.value()),
            }
        }
    }
}

/// Two-button toolbar that picks between surname A–Z and most-books sort axes.
#[component]
fn SortSelector(sort: Sort, on_sort: EventHandler<Sort>) -> Element {
    rsx! {
        div { class: "idx-sort",
            span { class: "label", "Sort" }
            button {
                class: "idx-btn",
                "aria-pressed": if sort == Sort::Name { "true" } else { "false" },
                "data-testid": "authors-sort-name",
                onclick: move |_| on_sort.call(Sort::Name),
                "Last name A\u{2013}Z"
            }
            button {
                class: "idx-btn",
                "aria-pressed": if sort == Sort::BookCount { "true" } else { "false" },
                "data-testid": "authors-sort-count",
                onclick: move |_| on_sort.call(Sort::BookCount),
                "Most books"
            }
        }
    }
}

/// A–Z (plus `#`) jump strip; present letters become same-page anchors targeting `id="letter-{L}"` sections.
#[component]
fn LetterNav(
    alphabet: Vec<char>,
    present_letters: std::collections::HashSet<char>,
    letter_count: usize,
    filtered_count: usize,
) -> Element {
    // Present letters become real `<a href="#letter-{frag}">` anchors so the
    // browser's native fragment scroll handles the jump (no JS); absent
    // letters stay as plain spans to avoid dead links.
    rsx! {
        div { class: "idx-letters",
            for l in alphabet.iter() {
                {
                    let has = present_letters.contains(l);
                    // `#` is not valid inside a URL fragment, so the
                    // non-alpha bucket gets a textual slug instead.
                    let frag = letter_frag(*l);
                    if has {
                        rsx! {
                            a {
                                class: "idx-letter idx-letter-on",
                                href: "#letter-{frag}",
                                "data-testid": "authors-letter-{frag}",
                                "{l}"
                            }
                        }
                    } else {
                        rsx! {
                            span { class: "idx-letter", "{l}" }
                        }
                    }
                }
            }
            span { class: "idx-letters-spacer" }
            span { class: "mono idx-letters-count",
                "{letter_count} letters \u{b7} {filtered_count} authors"
            }
        }
    }
}

fn render_author_card(a: &AuthorSummary, server_url: &str) -> Element {
    let accent = a.accent.clone().unwrap_or_else(|| "var(--accent)".into());
    let name = a.name.clone();
    let id = a.id;
    let count = a.book_count;
    let parts: Vec<&str> = name.rsplitn(2, ' ').collect();
    // `rsplitn(2)` reverses; last name is parts[0], rest is parts[1] when present.
    let (rest, last) = if parts.len() == 2 {
        (parts[1].to_string(), parts[0].to_string())
    } else {
        (String::new(), name.clone())
    };
    let initial = name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .next()
        .unwrap_or('?');

    rsx! {
        Link {
            to: Route::AuthorDetail { id },
            class: "idx-card idx-card-author",
            style: "--accent: {accent}",
            "data-testid": "author-card",
            // F1.11: swap the letter glyph for the cached profile photo
            // when one exists. `server_url` is empty on web (same-origin
            // fetch) and the configured backend URL on mobile so the
            // markup is portable. Editing the photo lives on the author
            // detail page — the index is read-only.
            if a.has_photo {
                img {
                    class: "idx-card-avatar idx-card-avatar--photo",
                    src: "{server_url}/api/authors/{id}/photo",
                    alt: "{name}",
                }
            } else {
                div { class: "idx-card-avatar", "{initial}" }
            }
            div { class: "idx-card-body",
                div { class: "idx-card-title",
                    if !rest.is_empty() {
                        span { class: "idx-card-name-rest", "{rest} " }
                    }
                    span { class: "idx-card-name-last", "{last}" }
                }
                div { class: "idx-card-stats",
                    div { class: "idx-stat",
                        div { class: "mono idx-stat-label", "Books" }
                        div { class: "idx-stat-value", "{count}" }
                    }
                }
            }
        }
    }
}

/// URL-fragment-safe slug for a letter glyph. `#` cannot appear inside a
/// fragment identifier — it would be parsed as a second fragment — so the
/// non-alpha bucket renders as `letter-hash` instead.
fn letter_frag(c: char) -> String {
    if c == '#' {
        "hash".to_string()
    } else {
        c.to_string()
    }
}

fn first_letter(a: &AuthorSummary) -> char {
    let raw = sort_key(a)
        .chars()
        .next()
        .unwrap_or('#')
        .to_uppercase()
        .next()
        .unwrap_or('#');
    // Anything not in A–Z (digits, punctuation, accented mononyms,
    // CJK, …) collapses into a single '#' bucket rendered after Z so
    // the alpha range stays uncluttered.
    if raw.is_ascii_alphabetic() {
        raw
    } else {
        '#'
    }
}

/// Whether the given sort key would land in the `#` bucket rather
/// than under A–Z. Used as the primary sort axis for `Sort::Name` so
/// the non-alpha tail trails the alphabet.
fn is_non_alpha_key(key: &str) -> bool {
    !key.chars()
        .next()
        .map(|c| c.is_ascii_alphabetic())
        .unwrap_or(false)
}

/// Surname-first key driving both alphabetical ordering and the letter
/// glyph above each section.
///
/// Three-stage derivation, in order:
///
/// 1. If `sort` follows the OPF "Surname, Given" convention (carries a
///    comma), use it verbatim — that's what the source metadata says.
/// 2. Else if the display `name` is already in "Surname, Given" form
///    (some Calibre dumps store it that way), use it verbatim too.
/// 3. Otherwise, treat the display name as "Given Surname" and
///    construct "Surname, Given" from `name.rsplitn(2, ' ')`.
///
/// We deliberately do **not** trust a comma-less `sort` value. A few
/// real-world dumps store just a surname (or an unrelated string like
/// a pseudonym) there, and using it blindly groups authors under the
/// wrong letter — e.g. "Erin A. Craig" with `sort = "Underwood"`
/// appearing under the U section. The order/letter stay coupled so
/// cards within a section sort the same way the section glyph reads.
fn sort_key(a: &AuthorSummary) -> String {
    if let Some(s) = a.sort.as_deref().filter(|s| !s.is_empty()) {
        if s.contains(',') {
            return s.to_string();
        }
    }
    if a.name.contains(',') {
        return a.name.clone();
    }
    let parts: Vec<&str> = a.name.rsplitn(2, ' ').collect();
    if parts.len() == 2 {
        format!("{}, {}", parts[0], parts[1])
    } else {
        a.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn author(name: &str, sort: Option<&str>) -> AuthorSummary {
        AuthorSummary {
            id: 0,
            name: name.into(),
            sort: sort.map(Into::into),
            book_count: 0,
            accent: None,
            has_photo: false,
        }
    }

    #[test]
    fn sort_key_uses_comma_form_sort_verbatim() {
        let a = author("Louisa May Alcott", Some("Alcott, Louisa May"));
        assert_eq!(sort_key(&a), "Alcott, Louisa May");
        assert_eq!(first_letter(&a), 'A');
    }

    #[test]
    fn sort_key_ignores_commaless_sort_and_falls_back_to_name() {
        // Regression: a comma-less `sort` value (some real-world dumps
        // stuff a pseudonym or bare surname here, e.g. "Underwood" for
        // an author whose display name is "Erin A. Craig") used to win,
        // grouping Erin under U. Now we treat it as garbage and derive
        // the surname from the display name instead.
        let a = author("Erin A. Craig", Some("Underwood"));
        assert_eq!(sort_key(&a), "Craig, Erin A.");
        assert_eq!(first_letter(&a), 'C');
    }

    #[test]
    fn sort_key_falls_back_to_name_when_sort_missing() {
        let a = author("Ada Lovelace", None);
        assert_eq!(sort_key(&a), "Lovelace, Ada");
        assert_eq!(first_letter(&a), 'L');
    }

    #[test]
    fn sort_key_treats_lastname_first_display_name_verbatim() {
        // Some Calibre dumps store the display name itself as
        // "Surname, Given". Keep it as-is — `rsplitn(2, ' ')` would
        // otherwise treat "Olivia" as the surname.
        let a = author("Atwater, Olivia", None);
        assert_eq!(sort_key(&a), "Atwater, Olivia");
        assert_eq!(first_letter(&a), 'A');
    }

    #[test]
    fn sort_key_handles_mononym() {
        let a = author("Plato", None);
        assert_eq!(sort_key(&a), "Plato");
        assert_eq!(first_letter(&a), 'P');
    }

    #[test]
    fn sort_key_handles_empty_sort_string() {
        // Explicit empty string in `sort` should be ignored the same
        // way as `None`.
        let a = author("Ada Lovelace", Some(""));
        assert_eq!(sort_key(&a), "Lovelace, Ada");
        assert_eq!(first_letter(&a), 'L');
    }

    #[test]
    fn first_letter_buckets_non_alpha_under_hash() {
        // Digits, punctuation, and non-Latin scripts collapse into the
        // single '#' section after Z. The test names use mononyms so
        // `sort_key` doesn't pull a Latin surname out of a multi-word
        // string (e.g. "1984 Editorial" would derive "Editorial, 1984"
        // and land under E — which is correct, but not what this test
        // is checking).
        //
        // We also avoid Latin diacritics here because their byte form
        // depends on source-file Unicode normalization — `chars().next()`
        // on NFD-encoded "Émile" returns plain `E`, which would land
        // under E. Either bucket is defensible; what matters is that
        // unambiguously-non-Latin starts land under '#'.
        assert_eq!(first_letter(&author("1984", None)), '#');
        assert_eq!(first_letter(&author("[Anonymous]", None)), '#');
        assert_eq!(first_letter(&author("\u{4E2D}\u{6587}", None)), '#');
    }

    #[test]
    fn is_non_alpha_key_classifies_keys() {
        assert!(!is_non_alpha_key("Alcott, Louisa May"));
        assert!(!is_non_alpha_key("zZz"));
        assert!(is_non_alpha_key("1984"));
        assert!(is_non_alpha_key("[Anonymous]"));
        assert!(is_non_alpha_key("\u{4E2D}\u{6587}"));
        assert!(is_non_alpha_key(""));
    }
}
