//! The structured query a search is run with.
//!
//! One string is not a query. "Dune Frank Herbert" handed to Open Library's
//! `title=` searches the author's name *inside the title field*, which ranks
//! five books written **about** Dune above the novel; the same string handed
//! to Hardcover's exact-title filter matches nothing at all. Keeping the
//! parts separate is what lets each provider be asked in its own terms.

use omnibus_shared::isbn::normalize_isbn;

/// What a caller knows about the book it is searching for.
///
/// Every field is optional and independently useful: the check-in ladder
/// searches with a title alone, the picker seeds title + author + whatever
/// ISBN the edit form already holds, and a reader who retypes the box gets a
/// title-only query because free text cannot honestly be split.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    /// The work's title, without the author appended.
    pub title: Option<String>,
    /// The primary author.
    pub author: Option<String>,
    /// A canonical ISBN-13. Already validated — [`SearchQuery::new`] discards
    /// anything `normalize_isbn` rejects rather than passing a malformed
    /// identifier to three different APIs.
    pub isbn13: Option<String>,
}

impl SearchQuery {
    /// Build from raw caller input, trimming blanks to `None` and keeping the
    /// ISBN only if it survives normalization.
    ///
    /// A bad ISBN is dropped rather than reported: it is a *hint* here, not
    /// the thing being looked up, and refusing the whole search because a
    /// stale field holds a typo would be worse than searching without it.
    pub fn new(title: Option<&str>, author: Option<&str>, isbn: Option<&str>) -> Self {
        Self {
            title: clean(title),
            author: clean(author),
            isbn13: clean(isbn).and_then(|raw| normalize_isbn(&raw).ok()),
        }
    }

    /// A title-only query — the honest reading of free text a reader typed.
    pub fn from_text(text: &str) -> Self {
        Self {
            title: clean(Some(text)),
            ..Self::default()
        }
    }

    /// The same query with the ISBN removed, for the retry that follows an
    /// ISBN lookup finding nothing.
    pub fn without_isbn(&self) -> Self {
        Self {
            isbn13: None,
            ..self.clone()
        }
    }

    /// The same query with the author removed, for the last retry.
    pub fn without_author(&self) -> Self {
        Self {
            author: None,
            isbn13: None,
            ..self.clone()
        }
    }

    /// Whether there is anything here worth sending to a provider.
    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.author.is_none() && self.isbn13.is_none()
    }

    /// Title and author joined for a provider whose search takes one free-text
    /// box (Google Books, Hardcover). The concatenation is right *there* —
    /// both rank a bare phrase well — and wrong for a fielded API, which is
    /// why it is spelled out here rather than done by the caller for everyone.
    pub fn as_text(&self) -> String {
        match (self.title.as_deref(), self.author.as_deref()) {
            (Some(t), Some(a)) => format!("{t} {a}"),
            (Some(t), None) => t.to_string(),
            (None, Some(a)) => a.to_string(),
            (None, None) => String::new(),
        }
    }
}

/// Trim, and treat whitespace-only as absent.
fn clean(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests;
