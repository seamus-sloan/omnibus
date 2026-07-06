//! Search results page — the full page reached from the command palette.
//! Reuses the `data::search_palette` RPC and groups hits by type (Books,
//! Authors, Series, Tags) with the matched term highlighted, an "On this
//! page" jump rail, and a tags-first ordering when the query matches tag
//! names.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{
    PaletteAuthorHit, PaletteBookHit, PaletteResults, PaletteSeriesHit, PaletteTagHit,
};

use crate::{data, use_server_url, Route};

/// Renders the full-page search results for the given query.
#[component]
pub fn SearchPage(query: String) -> Element {
    let server_url = use_server_url();
    let mut results: Signal<Option<PaletteResults>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    // See `BookDetailPage` for why `query` needs `use_reactive!`.
    let url = server_url.clone();
    let query_dep = query.clone();
    use_effect(use_reactive!(|query_dep| {
        let url = url.clone();
        spawn(async move {
            loading.set(true);
            match data::search_palette(&url, &query_dep).await {
                Ok(r) => {
                    results.set(Some(r));
                    error.set(None);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            loading.set(false);
        });
    }));

    let header = rsx! {
        nav { class: "breadcrumb",
            Link {
                to: Route::Landing {},
                "data-testid": "search-back",
                "Library"
            }
            span { class: "breadcrumb-sep", "\u{203a}" }
            span { "Search" }
        }
    };

    if loading() {
        return rsx! {
            section { class: "search-page",
                {header}
                p { class: "subtitle", "Searching\u{2026}" }
            }
        };
    }
    if let Some(msg) = error() {
        return rsx! {
            section { class: "search-page",
                {header}
                p { role: "alert", class: "subtitle", "{msg}" }
            }
        };
    }
    let Some(r) = results() else {
        return rsx! {
            section { class: "search-page",
                {header}
                p { class: "subtitle", "No results for \u{201c}{query}\u{201d}." }
            }
        };
    };

    rsx! {
        section { class: "search-page",
            {header}
            SearchResults { results: r, query: query.clone() }
        }
    }
}

/// Which result group a section renders. Drives both the main column order
/// and the "On this page" rail.
#[derive(Clone, Copy, PartialEq)]
enum Section {
    Books,
    Authors,
    Series,
    Tags,
}

impl Section {
    fn label(self) -> &'static str {
        match self {
            Section::Books => "Books",
            Section::Authors => "Authors",
            Section::Series => "Series",
            Section::Tags => "Tags",
        }
    }

    /// In-page anchor id used by the "On this page" jump links.
    fn anchor(self) -> &'static str {
        match self {
            Section::Books => "results-books",
            Section::Authors => "results-authors",
            Section::Series => "results-series",
            Section::Tags => "results-tags",
        }
    }

    fn count(self, r: &PaletteResults) -> u32 {
        match self {
            Section::Books => r.book_total,
            Section::Authors => r.author_total,
            Section::Series => r.series_total,
            Section::Tags => r.tag_total,
        }
    }
}

/// Grouped, highlighted result sections + an "On this page" rail for a loaded
/// [`PaletteResults`].
#[component]
fn SearchResults(results: PaletteResults, query: String) -> Element {
    let server_url = use_server_url();
    let r = results;
    let q = query;
    let total = r.total_count();

    if total == 0 {
        return empty_results(&r, &q);
    }

    let (order, tag_match) = section_order(&r, &q);
    let result_word = if total == 1 { "result" } else { "results" };

    rsx! {
        div { class: "search-head",
            div {
                div { class: "label", "Search results" }
                h1 { class: "search-title",
                    span { class: "search-title-n", "{total}" }
                    " {result_word} for \u{201c}"
                    {highlight(&q, &q)}
                    "\u{201d}"
                }
            }
            div { class: "search-controls",
                span { class: "label", "Sort" }
                button { class: "btn sm", "Relevance \u{2193}" }
                span { class: "label", "View" }
                button { class: "btn sm search-view-active", "Grid" }
                button { class: "btn ghost sm", "Table" }
            }
        }
        p { class: "search-summary mono", "data-testid": "search-result-count",
            {summary_line(&r)}
        }

        div { class: "search-layout",
            div { class: "search-main",
                for (i, sec) in order.iter().enumerate() {
                    div { key: "{sec.label()}", class: "search-section-slot",
                        if i > 0 {
                            div { class: "divider" }
                        }
                        {section_node(*sec, &r, &q, tag_match, &server_url)}
                    }
                }
            }
            {on_this_page(&order, &r, tag_match, &q)}
        }
    }
}

/// Empty-state header + zero-count summary for a query with no hits.
fn empty_results(r: &PaletteResults, q: &str) -> Element {
    rsx! {
        div { class: "search-head",
            div {
                div { class: "label", "Search results" }
                h1 { class: "search-title",
                    "No results for \u{201c}"
                    {highlight(q, q)}
                    "\u{201d}"
                }
            }
        }
        p { class: "subtitle", "data-testid": "search-result-count",
            "0 results \u{00b7} fts5 \u{00b7} {r.duration_ms}ms"
        }
    }
}

/// Non-empty section order plus whether the query matched a tag name. A tag-name
/// match leads with Tags (the second design artboard); otherwise the canonical
/// type order. Empty sections are filtered out. The bool is threaded on to the
/// section renderers so they can annotate a tag-driven result set.
fn section_order(r: &PaletteResults, q: &str) -> (Vec<Section>, bool) {
    let q_lower = q.to_lowercase();
    let tag_match = r
        .tags
        .iter()
        .any(|t| t.name.to_lowercase().contains(&q_lower));
    let canonical = [
        Section::Books,
        Section::Authors,
        Section::Series,
        Section::Tags,
    ];
    let tags_first = [
        Section::Tags,
        Section::Books,
        Section::Authors,
        Section::Series,
    ];
    let order: Vec<Section> = if tag_match { tags_first } else { canonical }
        .into_iter()
        .filter(|s| s.count(r) > 0)
        .collect();
    (order, tag_match)
}

// ── Section renderers ────────────────────────────────────────────────────

/// Dispatch one [`Section`] to its group renderer.
fn section_node(
    sec: Section,
    r: &PaletteResults,
    q: &str,
    tag_match: bool,
    server_url: &str,
) -> Element {
    match sec {
        Section::Books => books_group(r, q, tag_match, server_url),
        Section::Authors => authors_group(r, q),
        Section::Series => series_group(r, q),
        Section::Tags => tags_group(r, q, tag_match),
    }
}

/// Section heading: `LABEL · count` with an optional right-aligned hint.
fn section_head(sec: Section, count: u32, sub: Option<&str>) -> Element {
    rsx! {
        div { class: "search-section-head",
            div { class: "label", "{sec.label()}" }
            div { class: "mono search-section-count", "\u{00b7} {count}" }
            if let Some(sub) = sub {
                div { class: "search-section-sub", "{sub}" }
            }
        }
    }
}

fn books_group(r: &PaletteResults, q: &str, tag_match: bool, server_url: &str) -> Element {
    let sub = if tag_match {
        Some("in matched tags")
    } else {
        None
    };
    rsx! {
        section { id: "{Section::Books.anchor()}", class: "search-section",
            {section_head(Section::Books, r.book_total, sub)}
            div { class: "search-cover-grid",
                for book in r.books.iter().cloned() {
                    Link {
                        key: "{book.uuid}",
                        to: Route::BookDetail { uuid: book.uuid.clone() },
                        class: "search-cover-card",
                        "data-testid": "search-book-row",
                        {book_cover(server_url, &book)}
                        div { class: "search-cover-title", {highlight(&book.title, q)} }
                        div { class: "search-cover-sub", "{book.author_display}" }
                    }
                }
            }
        }
    }
}

fn authors_group(r: &PaletteResults, q: &str) -> Element {
    rsx! {
        section { id: "{Section::Authors.anchor()}", class: "search-section",
            {section_head(Section::Authors, r.author_total, None)}
            div { class: "search-card-grid",
                for author in r.authors.iter().cloned() {
                    {author_card(&author, q)}
                }
            }
        }
    }
}

fn author_card(author: &PaletteAuthorHit, q: &str) -> Element {
    let initial = author
        .name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    let books = format!("{} book{}", author.book_count, plural(author.book_count));
    rsx! {
        Link {
            key: "{author.id}",
            to: Route::AuthorDetail { id: author.id },
            class: "search-entity",
            "data-testid": "search-author-row",
            div { class: "search-avatar", "{initial}" }
            div { class: "search-entity-body",
                div { class: "search-entity-title", {highlight(&author.name, q)} }
                div { class: "search-entity-sub mono",
                    "{books}"
                    if let Some(ref lead) = author.lead_book_title {
                        " \u{00b7} incl. {lead}"
                    }
                }
            }
            span { class: "search-entity-arrow", "\u{2192}" }
        }
    }
}

fn series_group(r: &PaletteResults, q: &str) -> Element {
    rsx! {
        section { id: "{Section::Series.anchor()}", class: "search-section",
            {section_head(Section::Series, r.series_total, None)}
            div { class: "search-card-grid",
                for s in r.series.iter().cloned() {
                    {series_card(&s, q)}
                }
            }
        }
    }
}

fn series_card(s: &PaletteSeriesHit, q: &str) -> Element {
    rsx! {
        Link {
            key: "{s.id}",
            to: Route::SeriesDetail { id: s.id },
            class: "search-entity",
            "data-testid": "search-series-row",
            div { class: "search-avatar search-avatar-series", "\u{224b}" }
            div { class: "search-entity-body",
                div { class: "search-entity-title search-entity-title-serif", {highlight(&s.name, q)} }
                div { class: "search-entity-sub mono",
                    if let Some(ref lead) = s.lead_book_title {
                        "incl. {lead}"
                    } else {
                        "{s.book_count} book{plural(s.book_count)}"
                    }
                    if let Some(ref a) = s.author_display {
                        " \u{00b7} {a}"
                    }
                }
            }
            span { class: "search-entity-n mono", "{s.book_count}" }
        }
    }
}

fn tags_group(r: &PaletteResults, q: &str, tag_match: bool) -> Element {
    let sub = if tag_match {
        "matched in tag name"
    } else {
        "related to your matches"
    };
    let q_lower = q.to_lowercase();
    rsx! {
        section { id: "{Section::Tags.anchor()}", class: "search-section",
            {section_head(Section::Tags, r.tag_total, Some(sub))}
            div { class: "search-tags",
                for tag in r.tags.iter().cloned() {
                    {tag_chip(&tag, q, &q_lower)}
                }
            }
        }
    }
}

fn tag_chip(tag: &PaletteTagHit, q: &str, q_lower: &str) -> Element {
    let matched = tag.name.to_lowercase().contains(q_lower);
    let class = if matched {
        "chip search-tag search-tag-matched"
    } else {
        "chip search-tag"
    };
    rsx! {
        Link {
            key: "{tag.id}",
            to: Route::Search { query: format!("tag:{}", tag.name) },
            class: "{class}",
            "data-testid": "search-tag-row",
            {highlight(&tag.name, q)}
            span { class: "search-tag-count", " \u{00b7} {tag.book_count}" }
        }
    }
}

/// Right-rail "On this page" jump list plus a context card. A tag-name match
/// offers the tag cloud; everything else just lists the sections.
fn on_this_page(order: &[Section], r: &PaletteResults, tag_match: bool, q: &str) -> Element {
    rsx! {
        aside { class: "search-rail",
            div { class: "label search-rail-head", "On this page" }
            div { class: "search-rail-list",
                for (i, sec) in order.iter().enumerate() {
                    a {
                        key: "{sec.label()}",
                        href: "#{sec.anchor()}",
                        class: if i == 0 { "search-rail-item search-rail-item-active" } else { "search-rail-item" },
                        span { "{sec.label()}" }
                        span { class: "search-rail-count mono", "{sec.count(r)}" }
                    }
                }
            }
            if tag_match {
                div { class: "divider" }
                div { class: "search-refine",
                    div { class: "search-refine-title", "Browse by tag instead?" }
                    div { class: "search-refine-body",
                        "See the whole tag cloud and how \u{201c}{q}\u{201d} overlaps with neighbouring tags."
                    }
                    Link {
                        to: Route::TagCloud {},
                        class: "btn sm search-refine-btn",
                        "Open tag cloud \u{2192}"
                    }
                }
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// Book cover thumbnail for a palette hit, falling back to an accent-backed
/// initial plate when the book has no cover (mirrors the palette rows).
fn book_cover(server_url: &str, book: &PaletteBookHit) -> Element {
    if book.cover_url.is_some() {
        let url = crate::thumb_url(server_url, &book.uuid, "md");
        rsx! {
            img { class: "search-cover-thumb", src: "{url}", alt: "", loading: "lazy" }
        }
    } else {
        let initial = book
            .title
            .chars()
            .next()
            .unwrap_or('?')
            .to_uppercase()
            .to_string();
        let bg = book.accent.as_deref().unwrap_or("var(--bg-2)");
        rsx! {
            div { class: "search-cover-thumb search-cover-fallback", style: "background: {bg};",
                "{initial}"
            }
        }
    }
}

/// Render `text` with every case-insensitive occurrence of `q` wrapped in an
/// accented `<em class="search-hit">`. Returns the plain text untouched when
/// `q` is empty or doesn't occur, so non-matching fields stay free of stray
/// markup.
fn highlight(text: &str, q: &str) -> Element {
    let q_lower = q.to_lowercase();
    let qn = q_lower.chars().count();
    if qn == 0 {
        return rsx! { "{text}" };
    }
    let q_chars: Vec<char> = q_lower.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    // Compare against a per-char lowercased copy. One char → one char holds
    // for ASCII and the Latin script titles carry, which is all the search
    // box realistically sees; exotic multi-char lowercasings just won't match.
    let lower: Vec<char> = text_chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();

    let mut segments: Vec<(String, bool)> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < text_chars.len() {
        if i + qn <= text_chars.len() && lower[i..i + qn] == q_chars[..] {
            if !buf.is_empty() {
                segments.push((std::mem::take(&mut buf), false));
            }
            segments.push((text_chars[i..i + qn].iter().collect(), true));
            i += qn;
        } else {
            buf.push(text_chars[i]);
            i += 1;
        }
    }
    if !buf.is_empty() {
        segments.push((buf, false));
    }

    // No hit → emit the original string with no wrapper markup.
    if !segments.iter().any(|(_, hit)| *hit) {
        return rsx! { "{text}" };
    }

    rsx! {
        for (idx, (seg, hit)) in segments.into_iter().enumerate() {
            if hit {
                em { key: "{idx}", class: "search-hit", "{seg}" }
            } else {
                span { key: "{idx}", "{seg}" }
            }
        }
    }
}

/// Mono summary line under the header: per-category totals, then engine + time.
fn summary_line(r: &PaletteResults) -> String {
    let mut parts: Vec<String> = Vec::new();
    if r.book_total > 0 {
        let word = if r.book_total == 1 { "title" } else { "titles" };
        parts.push(format!("{} {word}", r.book_total));
    }
    if r.author_total > 0 {
        parts.push(format!(
            "{} author{}",
            r.author_total,
            plural(r.author_total)
        ));
    }
    if r.series_total > 0 {
        parts.push(format!("{} series", r.series_total));
    }
    if r.tag_total > 0 {
        parts.push(format!("{} tag{}", r.tag_total, plural(r.tag_total)));
    }
    parts.push("fts5".to_string());
    parts.push(format!("{}ms", r.duration_ms));
    parts.join(" \u{00b7} ")
}

fn plural(n: u32) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnibus_shared::{PaletteAuthorHit, PaletteBookHit, PaletteResults};

    #[test]
    fn plural_matches_count() {
        assert_eq!(plural(0), "s");
        assert_eq!(plural(1), "");
        assert_eq!(plural(2), "s");
    }

    #[test]
    fn total_count_sums_per_category_totals_not_capped_lengths() {
        // Vecs hold the 5-hit display cap; the totals carry the true counts.
        let r = PaletteResults {
            query: "x".into(),
            books: vec![PaletteBookHit::default()],
            authors: vec![PaletteAuthorHit::default(), PaletteAuthorHit::default()],
            series: vec![],
            tags: vec![],
            duration_ms: 0,
            book_total: 40,
            author_total: 2,
            series_total: 0,
            tag_total: 3,
        };
        assert_eq!(r.total_count(), 45);
    }

    #[test]
    fn summary_line_lists_present_categories_with_engine_and_time() {
        let r = PaletteResults {
            query: "wind".into(),
            duration_ms: 24,
            book_total: 2,
            author_total: 2,
            series_total: 2,
            tag_total: 4,
            ..Default::default()
        };
        assert_eq!(
            summary_line(&r),
            "2 titles \u{00b7} 2 authors \u{00b7} 2 series \u{00b7} 4 tags \u{00b7} fts5 \u{00b7} 24ms"
        );
    }

    #[test]
    fn summary_line_singularizes_and_omits_empty_categories() {
        let r = PaletteResults {
            query: "x".into(),
            duration_ms: 3,
            book_total: 0,
            author_total: 1,
            series_total: 0,
            tag_total: 1,
            ..Default::default()
        };
        assert_eq!(
            summary_line(&r),
            "1 author \u{00b7} 1 tag \u{00b7} fts5 \u{00b7} 3ms"
        );
    }
}
