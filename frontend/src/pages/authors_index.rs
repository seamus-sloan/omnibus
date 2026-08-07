//! Authors index page. Lists every author in the library as a browsable
//! surface, mirroring the `AuthorsIndex` design comp from
//! `screens/indices.jsx`.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{AuthorSummary, IndexSort};

use super::index_shell::{
    index_card_stats, index_page_early_return, use_index_page_shell, IndexFilterInput,
    IndexPageState, IndexSortToggle,
};
use crate::{data, index_prefs, use_server_url, Route};

/// Library-wide totals rendered in the header's subtitle. Grouped so the
/// header doesn't grow a long `total_*` prop list.
#[derive(Clone, PartialEq, Eq)]
struct AuthorIndexCounts {
    total_authors: usize,
    total_books: usize,
}

/// Owned output of the filter/sort/group-by-letter pipeline, memoized as a
/// single unit (mirrors `landing.rs`'s `visible = use_memo(...)`) so none of
/// the three steps reruns on a render the filter/sort/source didn't touch.
#[derive(Clone, PartialEq)]
struct AuthorGroups {
    filtered: Vec<AuthorSummary>,
    letters: Vec<(char, Vec<AuthorSummary>)>,
}

/// Filter + sort + alphabet-strip state for the header. The strip's
/// visibility, glyphs, and footnote totals all change together as the
/// filtered set changes, so they ride the same struct.
#[derive(Clone, PartialEq, Eq)]
struct AuthorIndexFilter {
    filter: String,
    sort: IndexSort,
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
    let fetch_url = server_url;
    let shell = use_index_page_shell(
        move || {
            let url = fetch_url.clone();
            async move { data::list_authors(&url).await }
        },
        || index_prefs::load().authors_sort,
    );
    let IndexPageState {
        items: authors,
        loading,
        error,
        mut filter,
        mut sort,
    } = shell;

    if let Some(early) = index_page_early_return(loading, error) {
        return early;
    }

    // Borrow the signal's contents instead of cloning. The list can hold
    // up to `INDEX_LIMIT` (10k) rows; copying the whole `Vec` on every
    // re-render (which fires on each keystroke in the filter input) would
    // be wasted work. `Signal::read` hands back a guard that derefs to
    // `&Vec<AuthorSummary>` and the rsx! tree only needs borrowed views.
    let all = authors.read();
    let total_authors = all.len();
    let any_in_library = !all.is_empty();
    drop(all);

    // Memoized separately from the filter/sort/group-by pipeline below: this
    // total is unaffected by the filter text or sort key, so tying it to
    // that memo would recompute it on every keystroke for no reason.
    let total_books = use_memo(move || authors.read().iter().map(|a| a.book_count).sum::<usize>());

    // Group by first letter of sort key (last name when available, else
    // name); only meaningful when sorting by name. Folded into the same
    // memo as filter/sort so none of the three steps reruns on a render
    // that didn't touch authors/filter/sort (mirrors `landing.rs`'s
    // `visible = use_memo(...)`).
    let show_letters = matches!(sort(), IndexSort::Name);
    let groups = use_memo(move || {
        let all = authors.read();
        compute_author_groups(&all, &filter.read(), sort())
    });
    let groups_ref = groups.read();
    let AuthorGroups { filtered, letters } = &*groups_ref;

    let counts = AuthorIndexCounts {
        total_authors,
        total_books: total_books(),
    };
    let filter_state = build_filter_state(filter(), sort(), show_letters, letters, filtered.len());

    rsx! {
        div { class: "idx-page",
            AuthorsIndexHeader {
                counts,
                filter_state,
                on_filter: move |v| filter.set(v),
                on_sort: move |s: IndexSort| {
                    sort.set(s);
                    let mut prefs = index_prefs::load();
                    prefs.authors_sort = s;
                    index_prefs::save(&prefs);
                },
            }
            {authors_index_body(filtered, letters, show_letters, any_in_library, &server_url_for_cards)}
        }
    }
}

/// Assembles the alphabet-strip + filter/sort state passed to the header.
/// The A–Z run (plus a single `#` bucket for any name whose
/// surname-equivalent doesn't start with an ASCII letter — digits,
/// punctuation, accented mononyms, … — rendered after Z so the eye lands on
/// the alpha range first) is built here since it's only needed for this one
/// struct.
fn build_filter_state(
    filter_text: String,
    sort: IndexSort,
    show_letters: bool,
    letters: &[(char, Vec<AuthorSummary>)],
    filtered_count: usize,
) -> AuthorIndexFilter {
    let alphabet: Vec<char> = ('A'..='Z').chain(std::iter::once('#')).collect();
    let present_letters: std::collections::HashSet<char> =
        letters.iter().map(|(l, _)| *l).collect();
    AuthorIndexFilter {
        filter: filter_text,
        sort,
        show_letters,
        alphabet,
        present_letters,
        letter_count: letters.len(),
        filtered_count,
    }
}

/// Filter → sort → group-by-letter pipeline, folded into one step so the
/// `use_memo` above reruns once per authors/filter/sort change rather than
/// three times (mirrors `landing.rs`'s `visible = use_memo(...)`).
fn compute_author_groups(all: &[AuthorSummary], query: &str, sort: IndexSort) -> AuthorGroups {
    let q = query.to_lowercase();
    let mut filtered = filter_authors(all, &q);
    sort_authors(&mut filtered, sort);
    let filtered: Vec<AuthorSummary> = filtered.into_iter().cloned().collect();
    let letters = group_by_letter(&filtered, matches!(sort, IndexSort::Name));
    AuthorGroups { filtered, letters }
}

/// Client-side name filter (case-insensitive substring match).
fn filter_authors<'a>(all: &'a [AuthorSummary], query: &str) -> Vec<&'a AuthorSummary> {
    if query.is_empty() {
        all.iter().collect()
    } else {
        all.iter()
            .filter(|a| a.name.to_lowercase().contains(query))
            .collect()
    }
}

/// Sorts `filtered` in place along the selected axis.
fn sort_authors(filtered: &mut [&AuthorSummary], sort: IndexSort) {
    match sort {
        // Within IndexSort::Name the primary axis is the sort_key bucket:
        // alpha first (`is_alpha_bucket` → false sorts before true),
        // non-alpha (digits, punctuation, accented mononyms, etc.)
        // trail at the end under the '#' section. Secondary axis is
        // the lowercased key itself so cards stay alphabetical within
        // their letter group.
        IndexSort::Name => filtered.sort_by(|a, b| {
            let ka = sort_key(a);
            let kb = sort_key(b);
            is_non_alpha_key(&ka)
                .cmp(&is_non_alpha_key(&kb))
                .then_with(|| ka.to_lowercase().cmp(&kb.to_lowercase()))
        }),
        IndexSort::BookCount => filtered.sort_by(|a, b| {
            b.book_count
                .cmp(&a.book_count)
                .then_with(|| sort_key(a).to_lowercase().cmp(&sort_key(b).to_lowercase()))
        }),
    }
}

/// Buckets `filtered` by first letter of `sort_key`; empty when `show_letters`
/// is false. Owned (not borrowed) so the result can live inside the
/// `use_memo` pipeline in `AuthorsIndexPage` alongside `filtered`.
fn group_by_letter(
    filtered: &[AuthorSummary],
    show_letters: bool,
) -> Vec<(char, Vec<AuthorSummary>)> {
    let mut letters: Vec<(char, Vec<AuthorSummary>)> = Vec::new();
    if show_letters {
        for a in filtered {
            let l = first_letter(a);
            match letters.last_mut() {
                Some((existing, group)) if *existing == l => group.push(a.clone()),
                _ => letters.push((l, vec![a.clone()])),
            }
        }
    }
    letters
}

/// Body section (plain fn, not `#[component]`, so it can borrow `filtered`
/// and `letters` directly instead of cloning them again for a child prop).
fn authors_index_body<'a>(
    filtered: &'a [AuthorSummary],
    letters: &'a [(char, Vec<AuthorSummary>)],
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

/// Header: hero, filter/sort toolbar, optional A–Z letter strip.
#[component]
fn AuthorsIndexHeader(
    counts: AuthorIndexCounts,
    filter_state: AuthorIndexFilter,
    on_filter: EventHandler<String>,
    on_sort: EventHandler<IndexSort>,
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
                IndexFilterInput {
                    filter,
                    placeholder: "Filter authors by name\u{2026}",
                    aria_label: "Filter authors by name",
                    testid: "authors-filter",
                    on_filter,
                }
                IndexSortToggle {
                    sort,
                    name_label: "Last name A\u{2013}Z",
                    testid_prefix: "authors",
                    on_sort,
                }
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
                                key: "{l}",
                                class: "idx-letter idx-letter-on",
                                href: "#letter-{frag}",
                                "data-testid": "authors-letter-{frag}",
                                "{l}"
                            }
                        }
                    } else {
                        rsx! {
                            span { key: "{l}", class: "idx-letter", "{l}" }
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
            // F1.11: swap the letter glyph for the cached profile photo when
            // one exists. `media_url` keeps the markup portable — same-origin
            // relative on web, server base + `?token=` on mobile so the
            // WebView's `<img>` fetch authenticates. Editing the photo lives
            // on the author detail page — the index is read-only.
            if a.has_photo {
                img {
                    class: "idx-card-avatar idx-card-avatar--photo",
                    src: crate::media_url(server_url, &format!("/api/authors/{id}/photo")),
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
                {index_card_stats(count)}
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
/// than under A–Z. Used as the primary sort axis for `IndexSort::Name` so
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
mod tests;
