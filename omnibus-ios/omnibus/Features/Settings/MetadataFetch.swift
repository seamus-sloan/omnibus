//  MetadataFetch.swift
//  The fetch-metadata sheet's stage machine and its pure decision logic —
//  which screen follows what, which candidate leads the list, and what one
//  provider field would change about the draft.
//
//  Kept out of the view for the same reason `MetadataDraft` is: the copyable
//  fields are *data* here, so the compare screen renders `EditionField.allCases`
//  and no row is written by hand. Adding a field is one case plus the arms the
//  compiler then demands — not three edits across three files, one of which is
//  silently forgotten.

import Foundation

/// Which screen the sheet is showing.
enum MetadataFetchStage: Equatable {
    /// Opened, nothing asked yet. The fields are filled in from the book but
    /// no request has gone out, so the reader can see exactly what is about to
    /// be searched — and drop the ISBN if they want the wider list — before
    /// anything is spent on it.
    case ready
    case searching
    case results
    /// The search itself failed. Distinct from a search that ran and found
    /// nothing, which is `.results` with an empty list and a per-source line
    /// saying why.
    case failed(String)
    case compare(ProviderEdition)
}

/// One field a source can offer and the editor can take.
///
/// Adding one is: a case here, and the arms
/// [`label`](MetadataFetchField/label), `current`, `sourceValue`, `original`,
/// `apply`, and `undo` then refuse to build without. Nothing in the compare
/// screen changes — it renders `allCases` and nothing else.
///
/// **Tags are deliberately absent.** A provider's subject list lands on
/// `genres` in this codebase; `subjects` is whatever the EPUB's own
/// `<dc:subject>` entries were, and overwriting that with a provider's
/// vocabulary would destroy the one field that describes *this file*.
enum MetadataFetchField: String, CaseIterable, Identifiable, Sendable {
    case title
    case authors
    case publisher
    case published
    case series
    case seriesIndex
    case isbn13
    case isbn10
    case printPages
    case genres
    case description

    var id: String { rawValue }

    /// The row heading, matching the editor's own label for the same field so
    /// a reader can see where a taken value will land.
    var label: String {
        switch self {
        case .title: "Title"
        case .authors: "Authors"
        case .publisher: "Publisher"
        case .published: "Published"
        case .series: "Series"
        case .seriesIndex: "Index"
        case .isbn13: "ISBN-13"
        case .isbn10: "ISBN-10"
        case .printPages: "Print Pages"
        case .genres: "Genres"
        case .description: "Summary"
        }
    }

    /// Whether this field's values want the room of a paragraph rather than a
    /// line. Only the summary, but expressed as a property so the card layout
    /// asks the field instead of matching on one case.
    var isProse: Bool { self == .description }

    /// What the *draft* currently holds — the staged value, not the saved one,
    /// so a card reflects a take the moment it happens.
    func current(_ draft: MetadataDraft) -> String {
        switch self {
        case .title: draft.title
        case .authors: Self.list(draft.authors)
        case .publisher: draft.publisher
        case .published: draft.published
        case .series: draft.series
        case .seriesIndex: draft.seriesIndex
        case .isbn13: draft.isbn13
        case .isbn10: draft.isbn10
        case .printPages: draft.printPages
        case .genres: Self.list(draft.genres)
        case .description: draft.description
        }
    }

    /// What the source offers, or the empty string when it offers nothing.
    ///
    /// The empty string is the whole safety property of this screen: it is
    /// what `isAvailable` reads, and an unavailable field can't be taken — so
    /// a provider that doesn't know a field can never blank out a value the
    /// reader already has.
    func sourceValue(_ edition: ProviderEdition) -> String {
        func text(_ value: String?) -> String {
            value?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        }
        switch self {
        case .title: return text(edition.title)
        case .authors: return Self.list(edition.authors)
        case .publisher: return text(edition.publisher)
        case .published: return text(edition.year)
        case .series: return text(edition.series)
        case .seriesIndex: return text(edition.seriesIndex)
        case .isbn13: return text(edition.isbn13)
        case .isbn10: return text(edition.isbn10)
        // A provider reporting 0 pages is normalized to `nil` upstream, so
        // anything here is a real count.
        case .printPages: return edition.pages.map(String.init) ?? ""
        case .genres: return Self.list(edition.genres)
        case .description: return text(edition.description)
        }
    }

    /// Whether this source has anything to offer for this field.
    func isAvailable(_ edition: ProviderEdition) -> Bool {
        !sourceValue(edition).isEmpty
    }

    /// What the book held when the editor loaded it — the same frozen baseline
    /// the Save button counts against.
    ///
    /// Formatted exactly like `current` so the two compare as strings; a
    /// mismatch in formatting here would report a field as edited for the
    /// shape of its rendering rather than for its value.
    func original(_ loaded: MetadataDraft) -> String {
        current(loaded)
    }

    /// Whether this field is currently carrying an unsaved change.
    ///
    /// Defined against the *baseline*, not against what this sheet happened to
    /// do — so it is true for a field edited directly in the form too, it
    /// survives leaving and re-entering the compare screen, and it can never
    /// disagree with the editor's own dirty check, which asks the same
    /// question.
    func isStaged(draft: MetadataDraft, loaded: MetadataDraft) -> Bool {
        current(draft) != original(loaded)
    }

    /// Whether the source would actually change this field.
    ///
    /// What the compare screen filters on: a card that says the same thing on
    /// both sides is one the reader has to read and then dismiss, and there
    /// are usually more of those than of the ones that matter. A field the
    /// source has no value for is *not* a difference — it can't be taken, so
    /// showing it would only be showing a dash beside a dead control.
    func differs(draft: MetadataDraft, edition: ProviderEdition) -> Bool {
        let source = sourceValue(edition)
        return !source.isEmpty && source != current(draft)
    }

    /// Stage the source's value into the draft.
    ///
    /// Staging only: the editor's Save button stays the single writer, so
    /// dirty tracking, validation, and the changed-fields-only payload keep
    /// working untouched. A field the source lacks is a no-op — the guard is
    /// here as well as on the control, because "take all" also comes through.
    func apply(to draft: inout MetadataDraft, from edition: ProviderEdition) {
        guard isAvailable(edition) else { return }
        switch self {
        case .title: draft.title = sourceValue(edition)
        // The list fields write the source's own array rather than the joined
        // string the card displays.
        case .authors: draft.authors = Self.nonBlank(edition.authors)
        case .publisher: draft.publisher = sourceValue(edition)
        case .published: draft.published = sourceValue(edition)
        case .series: draft.series = sourceValue(edition)
        case .seriesIndex: draft.seriesIndex = sourceValue(edition)
        case .isbn13: draft.isbn13 = sourceValue(edition)
        case .isbn10: draft.isbn10 = sourceValue(edition)
        case .printPages: draft.printPages = sourceValue(edition)
        case .genres: draft.genres = Self.nonBlank(edition.genres)
        case .description: draft.description = sourceValue(edition)
        }
    }

    /// Put this field back the way the book had it.
    func undo(in draft: inout MetadataDraft, to loaded: MetadataDraft) {
        switch self {
        case .title: draft.title = loaded.title
        case .authors: draft.authors = loaded.authors
        case .publisher: draft.publisher = loaded.publisher
        case .published: draft.published = loaded.published
        case .series: draft.series = loaded.series
        case .seriesIndex: draft.seriesIndex = loaded.seriesIndex
        case .isbn13: draft.isbn13 = loaded.isbn13
        case .isbn10: draft.isbn10 = loaded.isbn10
        case .printPages: draft.printPages = loaded.printPages
        case .genres: draft.genres = loaded.genres
        case .description: draft.description = loaded.description
        }
    }

    /// How a list-valued field renders on one side of a card.
    ///
    /// Blank entries are dropped before joining, so a list of them reads as
    /// absent rather than as the separator string — `["", ""]` would otherwise
    /// render, and stage, as ", ". Used by both sides so a value and its
    /// baseline are compared on equal terms.
    private static func list(_ values: [String]) -> String {
        nonBlank(values).joined(separator: ", ")
    }

    /// The list a list-valued field actually stages: trimmed, blanks dropped,
    /// so what is written matches what the card displayed.
    private static func nonBlank(_ values: [String]) -> [String] {
        values
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
    }
}

/// Pure builders and gates, separated from the view so they're testable
/// without a server — the same split `CheckInFlow` makes.
enum MetadataFetchFlow {
    /// Rendered wherever a value is absent, on either side of a card. One
    /// owner, so the screens can't drift onto different dashes.
    static let empty = "\u{2014}"

    // MARK: - Asking

    /// The request the three fields describe, or `nil` when they are all
    /// blank.
    ///
    /// No inference: what the reader can see in the fields is exactly what
    /// each provider is asked for. `query` still carries the composed phrase,
    /// because the REST front door accepts free text from clients that have no
    /// picker — but nothing here depends on it round-tripping.
    static func searchRequest(
        title: String, author: String, isbn: String, providers: [MetadataProvider]?
    ) -> EditionSearchRequest? {
        let title = title.nilIfBlank
        let author = author.nilIfBlank
        let isbn = isbn.nilIfBlank
        guard title != nil || author != nil || isbn != nil else { return nil }
        return EditionSearchRequest(
            query: [title, author].compactMap { $0 }.joined(separator: " "),
            title: title,
            author: author,
            isbn: isbn,
            providers: providers
        )
    }

    /// A slow hydrate must not overwrite a candidate the reader has since
    /// replaced, or reappear after they went back to the list.
    ///
    /// Keyed on `(source, providerRef)` rather than on the whole value,
    /// because the record that comes back is by definition not the one that
    /// went out.
    static func hydrateShouldApply(stage: MetadataFetchStage, asked: ProviderEdition) -> Bool {
        guard case let .compare(showing) = stage else { return false }
        return showing.source == asked.source && showing.providerRef == asked.providerRef
    }

    /// Fill this record's empty fields from the thinner search hit that named
    /// it, keeping every value the fetched record already carries.
    ///
    /// The detail record a provider serves for one edition is richer in some
    /// fields and *poorer* in others than the hit that named it — Open
    /// Library's edition record has the publisher and the printing's own page
    /// count but no subjects or first-publish year, and its search document is
    /// the mirror image. Re-fetching would otherwise blank whatever the list
    /// row had shown, which is the one thing a picker must never do. Mirrors
    /// `ProviderEdition::fill_missing_from` on the server.
    static func merged(fetched: ProviderEdition, thinner: ProviderEdition) -> ProviderEdition {
        var out = fetched
        // Both sides are checked for content, not just presence: providers
        // don't consistently trim these, and filling an absent field with a
        // blank one turns "this source didn't say" into "this source said
        // nothing", which renders as a present-but-empty value.
        func fill(_ slot: inout String?, _ from: String?) {
            guard slot?.nilIfBlank == nil, let value = from?.nilIfBlank else { return }
            slot = value
        }
        fill(&out.isbn13, thinner.isbn13)
        fill(&out.isbn10, thinner.isbn10)
        fill(&out.year, thinner.year)
        fill(&out.publisher, thinner.publisher)
        fill(&out.description, thinner.description)
        fill(&out.coverURL, thinner.coverURL)
        fill(&out.series, thinner.series)
        // A book number is a position *in* a series, so it may only cross over
        // when both records name the same one. A provider that files a book
        // under two series can answer two queries in two row orders, and
        // pairing one series' name with another's number is worse than
        // reporting no number at all.
        if out.series?.nilIfBlank == thinner.series?.nilIfBlank {
            fill(&out.seriesIndex, thinner.seriesIndex)
        }
        if out.title.nilIfBlank == nil { out.title = thinner.title }
        if out.authors.isEmpty { out.authors = thinner.authors }
        if out.pages == nil { out.pages = thinner.pages }
        if out.firstPublishYear == nil { out.firstPublishYear = thinner.firstPublishYear }
        if out.genres.isEmpty { out.genres = thinner.genres }
        // The handle the reader selected by, not the one this lookup happened
        // to mint: the sheet keys its selection on the value it already holds.
        out.providerRef = thinner.providerRef
        return out
    }

    // MARK: - Ordering

    /// Put candidates in an order derived from the candidates themselves.
    ///
    /// Two problems, one key. The fan-out answers provider by provider, so the
    /// raw list is "everything Open Library found, then everything Google
    /// Books found" — a source being slow, newly configured, or briefly down
    /// reshuffles the whole list under the reader. And within a provider the
    /// order is whatever it felt like, which for a well-known title is
    /// reliably four study guides above the novel.
    ///
    /// The server's relevance score leads when it sent one: it is computed
    /// from the candidate and the query alone — never from which provider
    /// answered — so a source dropping out removes its rows and moves nothing
    /// else. Word coverage is the tiebreak, and the whole order for a response
    /// that carries no scores. Mirrors `in_stable_order` in the web picker.
    static func ordered(_ editions: [ProviderEdition], query: String) -> [ProviderEdition] {
        let words = queryWords(query)
        return editions
            .map { OrderKey(edition: $0, words: words) }
            .sorted()
            .map(\.edition)
    }

    /// The sort key, as a comparable value — Swift has no `sort_by_cached_key`,
    /// and recomputing a lowercased title per comparison is what makes a naive
    /// `sorted(by:)` quadratic in string work rather than in comparisons.
    private struct OrderKey: Comparable {
        let edition: ProviderEdition
        /// Negated so the *highest* score sorts first, and defaulted so a
        /// candidate with no score sorts below every scored one rather than
        /// above them.
        let score: Int
        let incomplete: Bool
        let titleLength: Int
        let title: String
        let isbn: String
        let source: String
        let ref: String

        init(edition: ProviderEdition, words: [String]) {
            let title = edition.title.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
            self.edition = edition
            // Widened to `Int` before negating: `Int32.min` has no positive
            // counterpart, and a value that came off the wire must not be able
            // to trap.
            score = edition.relevance.map { -Int($0) } ?? Int(Int32.max)
            incomplete = !coversEveryWord(haystack(edition), words)
            titleLength = title.count
            self.title = title
            isbn = edition.isbn13 ?? ""
            source = edition.source.rawValue
            ref = edition.providerRef
        }

        /// Compared key by key rather than as one tuple: Swift's tuple
        /// comparison operators stop at six elements, and this key has seven.
        static func < (lhs: OrderKey, rhs: OrderKey) -> Bool {
            if lhs.score != rhs.score { return lhs.score < rhs.score }
            // `false` sorts first, so this reads "not a full match" — the rows
            // that account for the whole query lead.
            if lhs.incomplete != rhs.incomplete { return !lhs.incomplete }
            if lhs.titleLength != rhs.titleLength { return lhs.titleLength < rhs.titleLength }
            if lhs.title != rhs.title { return lhs.title < rhs.title }
            if lhs.isbn != rhs.isbn { return lhs.isbn < rhs.isbn }
            if lhs.source != rhs.source { return lhs.source < rhs.source }
            return lhs.ref < rhs.ref
        }

        static func == (lhs: OrderKey, rhs: OrderKey) -> Bool {
            lhs.score == rhs.score
                && lhs.incomplete == rhs.incomplete
                && lhs.titleLength == rhs.titleLength
                && lhs.title == rhs.title
                && lhs.isbn == rhs.isbn
                && lhs.source == rhs.source
                && lhs.ref == rhs.ref
        }
    }

    /// The text a query is matched against: title **and** authors.
    ///
    /// The seeded query is "title + primary author", so matching the title
    /// alone would rank every candidate that merely names the author in its
    /// title — a study guide — above the book itself, which of course doesn't
    /// repeat its own author in its title.
    private static func haystack(_ edition: ProviderEdition) -> String {
        "\(edition.title) \(edition.authors.joined(separator: " "))".lowercased()
    }

    /// Words that appear in so many titles that requiring them ranks nothing.
    /// Listed rather than inferred from length: "The" and "Ida" are both three
    /// letters and only one of them is noise.
    private static let stopWords: Set<String> = [
        "a", "an", "and", "by", "for", "from", "in", "of", "on", "or", "the", "to", "with",
    ]

    /// The query's words, lowercased and stripped of punctuation, minus the
    /// stop words and the initials a name leaves behind.
    static func queryWords(_ query: String) -> [String] {
        query
            .split(whereSeparator: \.isWhitespace)
            .map { word in
                String(word)
                    .trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
                    .lowercased()
            }
            .filter { $0.count > 1 && !stopWords.contains($0) }
    }

    /// Whether `haystack` accounts for every word. An empty word list matches
    /// nothing, so a blank query leaves the order to the later keys rather
    /// than declaring every row a perfect match.
    private static func coversEveryWord(_ haystack: String, _ words: [String]) -> Bool {
        !words.isEmpty && words.allSatisfy { haystack.contains($0) }
    }

    // MARK: - Reading a candidate

    /// How many fields this candidate would change, against the draft as it
    /// stands. Shown on the list row so the reader can tell the candidate that
    /// would rewrite the book from the one that already matches it.
    ///
    /// A lower bound, and honestly so: a search hit is thinner than the record
    /// behind it, and hydrate-on-select can only add fields.
    static func changeCount(edition: ProviderEdition, draft: MetadataDraft) -> Int {
        MetadataFetchField.allCases.filter { $0.differs(draft: draft, edition: edition) }.count
    }

    /// The candidate's cover URL, or `nil` when the provider gave nothing
    /// usable. Blank-but-present is the case worth catching: handed to an
    /// image loader it resolves against nothing and quietly fails.
    static func coverURL(_ edition: ProviderEdition) -> String? {
        guard let url = edition.coverURL?.nilIfBlank else { return nil }
        return CheckInFlow.isExternalURL(url) ? url : nil
    }

    /// A candidate row's second line: authors, or a dash when the source named
    /// none.
    static func authorsLine(_ edition: ProviderEdition) -> String {
        edition.authorDisplay.nilIfBlank ?? empty
    }

    /// A candidate row's third line: year, publisher, ISBN — with the parts
    /// the provider left empty dropped rather than rendered as gaps.
    ///
    /// All three can be absent at once. A Hardcover search hit describes a
    /// *work*, so it names no printing, no publisher, and no edition ISBN; the
    /// row is still worth showing (its title, authors, and cover are exactly
    /// what the reader is choosing between) and hydrate-on-select fills the
    /// rest in.
    static func imprintLine(_ edition: ProviderEdition) -> String {
        let parts = [edition.year, edition.publisher, edition.isbn13]
            .compactMap { $0?.nilIfBlank }
        return parts.isEmpty ? empty : parts.joined(separator: " \u{b7} ")
    }

    // MARK: - Per-source status

    /// What one source contributed, in as few words as the distinction
    /// allows, and whether it reads as a problem.
    static func sourceStatus(_ status: ProviderSearchStatus) -> (text: String, isProblem: Bool) {
        switch status {
        case .answered(0): ("nothing", false)
        case let .answered(count): ("\(count)", false)
        case .notConfigured: ("not set up", false)
        case .failed: ("unavailable", true)
        // Worded as what *we* are doing, not as a promise about the source.
        // "retry in 10m" reads as advice — wait and it will work — and for the
        // commonest case that is false: a keyless Google Books answers 429 for
        // a quota metric of "queries per day", so it refuses until midnight
        // Pacific however long anyone waits. What is true either way is that
        // this source sits the next few minutes out.
        case let .throttled(secs): ("paused \(humanWait(secs))", true)
        case .unknown: (empty, false)
        }
    }

    /// A cooldown as a reader would say it. Rounded up, and never "0s" — the
    /// point of the number is roughly how long this source sits out, not its
    /// precision.
    static func humanWait(_ seconds: UInt64) -> String {
        seconds <= 90 ? "\(max(1, seconds))s" : "\((seconds + 59) / 60)m"
    }

    /// The line under the compare screen's take-all bar, or `nil` when this
    /// edition would change nothing.
    static func takeAllLabel(changes: Int) -> String? {
        switch changes {
        case 0: nil
        case 1: "Take 1 field"
        default: "Take all \(changes) fields"
        }
    }

    /// What the editor says after the sheet closes, or `nil` when nothing was
    /// staged while it was open. Names the source, because "4 fields changed"
    /// with no attribution is the state a reader can't audit.
    static func stagedNote(count: Int, source: MetadataProvider) -> String? {
        guard count > 0 else { return nil }
        let fields = count == 1 ? "1 field" : "\(count) fields"
        return "Staged \(fields) from \(source.displayName). Press Save to keep them."
    }
}
