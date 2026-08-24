//  MetadataFetchTests.swift
//  The fetch-metadata sheet's pure logic: what a request asks, what order the
//  candidates come in, what one field would change, and what a per-source
//  status reads as.

import Foundation
import Testing

@testable import omnibus

// MARK: - Fixtures

private func edition(
    title: String = "Dune",
    isbn13: String? = "9780441013593",
    source: MetadataProvider = .openLibrary,
    authors: [String] = ["Frank Herbert"]
) -> ProviderEdition {
    ProviderEdition(
        source: source,
        providerRef: "\(source.rawValue)-\(isbn13 ?? title)",
        isbn13: isbn13,
        isbn10: nil,
        title: title,
        authors: authors,
        year: "1965",
        pages: 412,
        publisher: "Chilton Books",
        description: nil,
        coverURL: nil,
        series: nil,
        seriesIndex: nil,
        firstPublishYear: nil,
        genres: [],
        relevance: nil
    )
}

private func draft(title: String = "Dune", publisher: String = "Ace") -> MetadataDraft {
    var draft = MetadataDraft()
    draft.title = title
    draft.authors = ["Frank Herbert"]
    draft.publisher = publisher
    return draft
}

// MARK: - Asking

struct MetadataFetchRequestTests {
    @Test func searchRequestKeepsTheThreeFieldsApart() {
        // The whole point of the structured request: Open Library gets a
        // title/author pair rather than one flattened phrase searched inside
        // the title field.
        let request = MetadataFetchFlow.searchRequest(
            title: "Dune", author: "Frank Herbert", isbn: "9780441013593", providers: nil
        )
        #expect(request?.title == "Dune")
        #expect(request?.author == "Frank Herbert")
        #expect(request?.isbn == "9780441013593")
        #expect(request?.query == "Dune Frank Herbert")
    }

    @Test func searchRequestIsNilWhenEveryFieldIsBlank() {
        #expect(
            MetadataFetchFlow.searchRequest(
                title: "  ", author: "", isbn: "\n", providers: nil
            ) == nil
        )
    }

    @Test func searchRequestAllowsAnIsbnOnlyQuery() {
        // The strongest question any provider takes, and one with no free
        // text to compose a `query` from at all.
        let request = MetadataFetchFlow.searchRequest(
            title: "", author: "", isbn: "9780441013593", providers: nil
        )
        #expect(request != nil)
        #expect(request?.query == "")
        #expect(request?.title == nil)
    }

    @Test func searchRequestTrimsEachFieldBeforeSending() {
        let request = MetadataFetchFlow.searchRequest(
            title: "  Dune  ", author: " Frank Herbert ", isbn: " ", providers: nil
        )
        #expect(request?.title == "Dune")
        #expect(request?.author == "Frank Herbert")
        #expect(request?.isbn == nil)
    }

    @Test func searchRequestEncodesTheWireFieldNames() throws {
        let request = try #require(
            MetadataFetchFlow.searchRequest(
                title: "Dune", author: "", isbn: "", providers: [.googleBooks]
            )
        )
        let data = try JSONEncoder().encode(request)
        let json = try #require(
            try JSONSerialization.jsonObject(with: data) as? [String: Any]
        )
        #expect(json["title"] as? String == "Dune")
        #expect(json["providers"] as? [String] == ["google_books"])
        // Absent, not null: the server's `Option` fields skip when empty, and
        // an explicit null would fail nothing but says something different.
        #expect(json["isbn"] == nil)
        #expect(json["author"] == nil)
    }
}

// MARK: - Ordering

struct MetadataFetchOrderingTests {
    @Test func orderIsIndependentOfTheOrderProvidersAnsweredIn() {
        // The property that matters: the same set sorts the same way however
        // the fan-out happened to concatenate it.
        let a = edition(title: "Dune", isbn13: "1", source: .openLibrary)
        let b = edition(title: "Dune", isbn13: "1", source: .googleBooks)
        let c = edition(title: "Neuromancer", isbn13: "2", source: .hardcover)

        let one = MetadataFetchFlow.ordered([a, b, c], query: "Dune")
        let two = MetadataFetchFlow.ordered([c, b, a], query: "Dune")
        let three = MetadataFetchFlow.ordered([b, a, c], query: "Dune")
        #expect(one == two)
        #expect(two == three)
    }

    @Test func orderLeadsWithTheTitlesThatAccountForTheWholeQuery() {
        // The observed failure this key exists for: a well-known title comes
        // back with study guides above the book itself.
        let sorted = MetadataFetchFlow.ordered(
            [
                edition(
                    title: "A Study Guide for Ursula K. LeGuin's The Left Hand of Darkness",
                    isbn13: "1", authors: ["Gale, Cengage Learning"]
                ),
                edition(title: "Gender under the Viking Age", isbn13: "2", authors: ["Rusty Brient"]),
                edition(
                    title: "The Left Hand of Darkness", isbn13: "3", authors: ["Ursula K. Le Guin"]
                ),
            ],
            query: "The Left Hand of Darkness Ursula K. Le Guin"
        )
        #expect(sorted.first?.title == "The Left Hand of Darkness")
        #expect(sorted.last?.title == "Gender under the Viking Age")
    }

    @Test func orderLeadsWithTheServersRelevanceScore() {
        // The score is derived from the candidate and the query alone, so it
        // is safe to lead with: a provider dropping out removes its rows and
        // reorders nothing.
        var weak = edition(title: "Dune", isbn13: "1")
        weak.relevance = 320
        var strong = edition(title: "A Study Guide", isbn13: "2", source: .googleBooks)
        strong.relevance = 1000

        let ordered = MetadataFetchFlow.ordered([weak, strong], query: "dune")
        #expect(ordered.first?.relevance == 1000)
    }

    @Test func orderSortsAnUnscoredCandidateBelowEveryScoredOne() {
        let unscored = edition(title: "Dune", isbn13: "1")
        var scored = edition(title: "Dune Messiah", isbn13: "2", source: .googleBooks)
        scored.relevance = 100

        let ordered = MetadataFetchFlow.ordered([unscored, scored], query: "dune")
        #expect(ordered.first?.relevance == 100)
        #expect(ordered.last?.relevance == nil)
    }

    @Test func orderSurvivesTheExtremeRelevanceAWireValueCouldCarry() {
        // `Int32.min` has no positive counterpart; negating it in place would
        // trap and take the results screen down with it.
        var extreme = edition(title: "Dune", isbn13: "1")
        extreme.relevance = Int32.min
        let ordered = MetadataFetchFlow.ordered([extreme, edition(title: "Other", isbn13: "2")], query: "dune")
        #expect(ordered.count == 2)
    }

    @Test func queryWordsDropsTheConnectivesAndInitials() {
        #expect(MetadataFetchFlow.queryWords("The Left Hand of Darkness") == ["left", "hand", "darkness"])
        // "K." carries no signal and would fail every haystack that spells the
        // name differently.
        #expect(MetadataFetchFlow.queryWords("Ursula K. Le Guin") == ["ursula", "le", "guin"])
    }

    @Test func orderFallsBackToTheEditionsOwnFieldsForABlankQuery() {
        // No query words means nothing is a match, so the order must come from
        // the later keys rather than every row tying at the top.
        let sorted = MetadataFetchFlow.ordered(
            [edition(title: "Zebra", isbn13: "1"), edition(title: "Alpha", isbn13: "2")],
            query: "   "
        )
        #expect(sorted.first?.title == "Alpha")
    }
}

// MARK: - Fields

struct MetadataFetchFieldTests {
    @Test func takingAFieldStagesOnlyThatField() {
        var current = draft()
        let loaded = current
        var candidate = edition()
        candidate.publisher = "Chilton Books"

        MetadataFetchField.publisher.apply(to: &current, from: candidate)
        #expect(current.publisher == "Chilton Books")
        #expect(current.title == loaded.title)
        #expect(MetadataFetchField.publisher.isStaged(draft: current, loaded: loaded))
        #expect(!MetadataFetchField.title.isStaged(draft: current, loaded: loaded))
    }

    @Test func aFieldTheSourceDoesNotKnowCanNeverBlankTheValueYouHave() {
        // The whole safety property of the compare screen.
        var current = draft(publisher: "Ace")
        var candidate = edition()
        candidate.publisher = nil
        candidate.description = "   "

        MetadataFetchField.publisher.apply(to: &current, from: candidate)
        MetadataFetchField.description.apply(to: &current, from: candidate)
        #expect(current.publisher == "Ace")
        #expect(current.description == "")
        #expect(!MetadataFetchField.publisher.isAvailable(candidate))
        #expect(!MetadataFetchField.publisher.differs(draft: current, edition: candidate))
    }

    @Test func undoPutsAFieldBackToWhatTheBookHad() {
        let loaded = draft(publisher: "Ace")
        var current = loaded
        var candidate = edition()
        candidate.publisher = "Chilton Books"

        MetadataFetchField.publisher.apply(to: &current, from: candidate)
        MetadataFetchField.publisher.undo(in: &current, to: loaded)
        #expect(current.publisher == "Ace")
        #expect(!MetadataFetchField.publisher.isStaged(draft: current, loaded: loaded))
    }

    @Test func listFieldsStageTheSourcesOwnEntriesWithBlanksDropped() {
        var current = draft()
        var candidate = edition()
        candidate.authors = [" Frank Herbert ", "", "Brian Herbert"]
        candidate.genres = ["Science Fiction", "  "]

        MetadataFetchField.authors.apply(to: &current, from: candidate)
        MetadataFetchField.genres.apply(to: &current, from: candidate)
        #expect(current.authors == ["Frank Herbert", "Brian Herbert"])
        #expect(current.genres == ["Science Fiction"])
    }

    @Test func aListOfBlanksReadsAsAbsentRatherThanAsTheSeparator() {
        // `["", ""]` would otherwise render — and stage — as ", ".
        var candidate = edition()
        candidate.authors = ["", "  "]
        #expect(MetadataFetchField.authors.sourceValue(candidate) == "")
        #expect(!MetadataFetchField.authors.isAvailable(candidate))
    }

    @Test func printPagesTakesTheEditionsCountAsText() {
        var current = draft()
        var candidate = edition()
        candidate.pages = 412
        MetadataFetchField.printPages.apply(to: &current, from: candidate)
        #expect(current.printPages == "412")
    }

    @Test func printPagesRefusesACountTheSaveWouldThenReject() throws {
        // `save()` validates before it does anything else, so one out-of-range
        // page count staged from a candidate would refuse the whole save and
        // discard every other field taken with it. Providers only filter
        // `> 0`; the bound has to be applied here.
        var current = draft()
        var candidate = edition()
        candidate.pages = MetadataDraft.printPagesMax + 1

        #expect(!MetadataFetchField.printPages.isAvailable(candidate))
        #expect(!MetadataFetchField.printPages.differs(draft: current, edition: candidate))
        MetadataFetchField.printPages.apply(to: &current, from: candidate)
        #expect(current.printPages == "")

        // The bound is inclusive at the top, so a real count still comes over.
        candidate.pages = MetadataDraft.printPagesMax
        MetadataFetchField.printPages.apply(to: &current, from: candidate)
        #expect(current.printPages == String(MetadataDraft.printPagesMax))
        // And what it stages is what the save's own parser accepts, which is
        // the property this bound exists to hold.
        let parsed = try MetadataDraft.parsePrintPages(current.printPages)
        #expect(parsed == MetadataDraft.printPagesMax)
    }

    @Test func aComposedQueryIsCappedToWhatTheServerAccepts() {
        // `EditionSearchRequest::validate` checks `query` first, so an
        // over-long composition 400s with a message naming a field the sheet
        // has no control for.
        let request = MetadataFetchFlow.searchRequest(
            title: String(repeating: "a", count: 400), author: "Frank Herbert", isbn: "",
            providers: nil
        )
        #expect(request?.query.count == MetadataFetchFlow.searchQueryMaxLength)
        // The structured fields are untouched — they have their own cap, and
        // they are what the providers are actually asked with.
        #expect(request?.title?.count == 400)
    }

    @Test func changeCountIgnoresFieldsThatAlreadyAgree() {
        var current = draft(title: "Dune", publisher: "Ace")
        // Everything the source is silent on is stripped, so the count under
        // test is exactly the one field left.
        var candidate = edition(title: "Dune", isbn13: nil)
        candidate.publisher = "Chilton Books"
        candidate.year = nil
        candidate.pages = nil

        // Title agrees, publisher differs — and a field the source is silent
        // on is not a difference, because it can't be taken.
        #expect(MetadataFetchFlow.changeCount(edition: candidate, draft: current) == 1)

        MetadataFetchField.publisher.apply(to: &current, from: candidate)
        #expect(MetadataFetchFlow.changeCount(edition: candidate, draft: current) == 0)
    }

    @Test func everyFieldRoundTripsThroughAllSixAccessors() {
        // `allCases` is what the compare screen renders, so a case that
        // compiles but can't be read, taken, or undone would show as a dead
        // card rather than as a build failure.
        let loaded = draft()
        var candidate = edition()
        candidate.isbn10 = "0441013597"
        candidate.series = "Dune"
        candidate.seriesIndex = "1"
        candidate.description = "A desert planet."
        candidate.genres = ["Science Fiction"]

        for field in MetadataFetchField.allCases {
            var current = loaded
            #expect(!field.label.isEmpty)
            _ = field.original(loaded)
            guard field.isAvailable(candidate) else { continue }
            field.apply(to: &current, from: candidate)
            #expect(field.current(current) == field.sourceValue(candidate))
            field.undo(in: &current, to: loaded)
            #expect(field.current(current) == field.original(loaded))
        }
    }
}

// MARK: - Hydrate

struct MetadataFetchHydrateTests {
    @Test func aLateHydrateIsDiscardedOnceAnotherCandidateIsShowing() {
        let asked = edition(title: "Dune", isbn13: "1")
        let other = edition(title: "Dune Messiah", isbn13: "2")
        #expect(MetadataFetchFlow.hydrateShouldApply(stage: .compare(asked), asked: asked))
        #expect(!MetadataFetchFlow.hydrateShouldApply(stage: .compare(other), asked: asked))
        // And after going back to the list, where there is nothing to apply to.
        #expect(!MetadataFetchFlow.hydrateShouldApply(stage: .results, asked: asked))
    }

    @Test func mergingFillsWhatTheDetailRecordLacksAndTakesNothingAway() {
        // Open Library's edition record has the publisher and the printing's
        // page count but no subjects or first-publish year; its search
        // document is the mirror image. A re-fetch must not blank what the
        // list row had shown.
        var thin = edition()
        thin.genres = ["Science Fiction"]
        thin.firstPublishYear = 1965
        thin.coverURL = "https://covers.example/1.jpg"
        thin.publisher = nil

        var fetched = edition()
        fetched.providerRef = "a-different-handle"
        fetched.genres = []
        fetched.firstPublishYear = nil
        fetched.coverURL = nil
        fetched.publisher = "Chilton Books"

        let merged = MetadataFetchFlow.merged(fetched: fetched, thinner: thin)
        #expect(merged.genres == ["Science Fiction"])
        #expect(merged.firstPublishYear == 1965)
        #expect(merged.coverURL == "https://covers.example/1.jpg")
        // The fetched record wins every conflict: it is the record fetched for
        // *this* edition, where the search hit may describe the work.
        #expect(merged.publisher == "Chilton Books")
        // And the handle stays the one the reader selected by, since the sheet
        // keys its staleness check on it.
        #expect(merged.providerRef == thin.providerRef)
    }

    @Test func mergingRefusesASeriesNumberFromADifferentSeries() {
        // Pairing one series' name with another's number is worse than
        // reporting no number at all.
        var thin = edition()
        thin.series = "The Dune Chronicles"
        thin.seriesIndex = "1"

        var fetched = edition()
        fetched.series = "Great Science Fiction"
        fetched.seriesIndex = nil

        let merged = MetadataFetchFlow.merged(fetched: fetched, thinner: thin)
        #expect(merged.series == "Great Science Fiction")
        #expect(merged.seriesIndex == nil)
    }

    @Test func mergingRestoresTheFieldsAWorkLevelDetailRecordDoesNotCarry() {
        // `openlibrary::by_ref` answers a work record: a title, with
        // `authors: []`, `pages: nil`, `publisher: nil`, `isbn13: nil`. That is
        // the path `hydrate_edition` takes for any candidate with no ISBN —
        // routine for pre-ISBN works and translations — so without these
        // restores the compare screen would show a dash for fields the list row
        // had just displayed, and their Take controls would go inert.
        var thin = edition(title: "Dune", isbn13: "9780441013593")
        thin.authors = ["Frank Herbert"]
        thin.pages = 412

        var fetched = edition(title: "", isbn13: nil)
        fetched.authors = []
        fetched.pages = nil
        fetched.publisher = nil
        fetched.year = nil

        let merged = MetadataFetchFlow.merged(fetched: fetched, thinner: thin)
        #expect(merged.title == "Dune")
        #expect(merged.authors == ["Frank Herbert"])
        #expect(merged.pages == 412)
        #expect(merged.isbn13 == "9780441013593")
        #expect(merged.publisher == "Chilton Books")
        #expect(merged.year == "1965")
    }

    @Test func aBlankFieldOnTheThinnerRecordIsNotFilledIn() {
        // Filling an absent field with a blank one turns "this source didn't
        // say" into "this source said nothing", which renders as a
        // present-but-empty value.
        var thin = edition()
        thin.publisher = "   "
        var fetched = edition()
        fetched.publisher = nil

        #expect(MetadataFetchFlow.merged(fetched: fetched, thinner: thin).publisher == nil)
    }

    @Test func aMissIsDecodedAsNoEditionRatherThanAsAFailure() throws {
        // `POST /api/metadata/editions/hydrate` answers a bare `null` when the
        // provider no longer knows the candidate, and the sheet absorbs that
        // by keeping the list row it already has.
        let decoded = try JSONDecoder().decode(
            ProviderEdition?.self, from: Data("null".utf8)
        )
        #expect(decoded == nil)
    }
}

// MARK: - Wire decoding

struct MetadataFetchDecodingTests {
    @Test func aSearchResponseDecodesEveryStatusKindDistinctly() throws {
        let json = """
        {
          "editions": [],
          "sources": [
            {"provider": "open_library", "display_name": "Open Library",
             "status": {"kind": "answered", "count": 8}},
            {"provider": "google_books", "display_name": "Google Books",
             "status": {"kind": "failed", "message": "timed out"}},
            {"provider": "hardcover", "display_name": "Hardcover",
             "status": {"kind": "not_configured"}},
            {"provider": "open_library", "display_name": "Throttled Source",
             "status": {"kind": "throttled", "retry_after_secs": 540}},
            {"provider": "book_brainz", "display_name": "Future Source",
             "status": {"kind": "invented_later"}}
          ]
        }
        """
        let response = try JSONDecoder().decode(
            EditionSearchResponse.self, from: Data(json.utf8)
        )
        #expect(response.sources.count == 5)
        #expect(response.sources[0].status == .answered(count: 8))
        #expect(response.sources[1].status == .failed(message: "timed out"))
        #expect(response.sources[2].status == .notConfigured)
        // The one arm with a hand-written CodingKey and a UInt64 payload, so
        // the one where a decode can actually go wrong.
        #expect(response.sources[3].status == .throttled(retryAfterSecs: 540))
        // And an arm this build has never heard of. `init(from:)` throws, and
        // one bad status row would fail the whole response — losing the
        // results every other provider did return.
        #expect(response.sources[4].status == .unknown)
        #expect(MetadataFetchFlow.sourceStatus(.unknown).text == MetadataFetchFlow.empty)
        #expect(!MetadataFetchFlow.sourceStatus(.unknown).isProblem)
    }

    @Test func anUnknownProviderStillDecodesAndStillNamesItself() throws {
        // A server that grows a fourth source must not fail this client's
        // decode of the other three.
        let json = """
        {"provider": "book_brainz", "display_name": "BookBrainz",
         "status": {"kind": "answered", "count": 1}}
        """
        let source = try JSONDecoder().decode(ProviderSearchSource.self, from: Data(json.utf8))
        #expect(source.provider.rawValue == "book_brainz")
        #expect(source.provider.displayName == "Book Brainz")
    }

    @Test func anEditionDecodesTheSnakeCasedWireFields() throws {
        let json = """
        {"source": "hardcover", "provider_ref": "4212", "title": "Dune",
         "authors": ["Frank Herbert"], "year": null, "pages": null,
         "publisher": null, "description": null, "cover_url": null,
         "series": "Dune", "series_index": "1", "first_publish_year": 1965,
         "genres": ["Science Fiction"], "relevance": 940}
        """
        let edition = try JSONDecoder().decode(ProviderEdition.self, from: Data(json.utf8))
        #expect(edition.providerRef == "4212")
        #expect(edition.seriesIndex == "1")
        #expect(edition.firstPublishYear == 1965)
        #expect(edition.relevance == 940)
        // A candidate with no ISBN is expected, not a parse failure: a search
        // hit is very often a work rather than a printing.
        #expect(edition.isbn13 == nil)
    }

    @Test func aHydrateRequestEncodesTheProviderRefUnderItsWireName() throws {
        let data = try JSONEncoder().encode(
            EditionHydrateRequest(source: .openLibrary, providerRef: "/works/OL45883W", isbn13: nil)
        )
        let json = try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
        #expect(json["provider_ref"] as? String == "/works/OL45883W")
        #expect(json["source"] as? String == "open_library")
        #expect(json["isbn13"] == nil)
    }
}

// MARK: - Per-source status

struct MetadataFetchStatusTests {
    @Test func statusReadsDifferentlyForEmptyUnconfiguredAndFailed() {
        // The point of the type: distinct causes, distinct reads — collapsing
        // them makes an outage look like a miss.
        let empty = MetadataFetchFlow.sourceStatus(.answered(count: 0)).text
        let unconfigured = MetadataFetchFlow.sourceStatus(.notConfigured).text
        let failed = MetadataFetchFlow.sourceStatus(.failed(message: "boom")).text
        #expect(empty != unconfigured)
        #expect(unconfigured != failed)
        #expect(empty != failed)
    }

    @Test func onlyAFailureOrACooldownReadsAsAProblem() {
        #expect(MetadataFetchFlow.sourceStatus(.failed(message: "boom")).isProblem)
        #expect(MetadataFetchFlow.sourceStatus(.throttled(retryAfterSecs: 600)).isProblem)
        #expect(!MetadataFetchFlow.sourceStatus(.notConfigured).isProblem)
        #expect(!MetadataFetchFlow.sourceStatus(.answered(count: 0)).isProblem)
    }

    @Test func statusIsABareCountWhenASourceAnswered() {
        #expect(MetadataFetchFlow.sourceStatus(.answered(count: 8)).text == "8")
    }

    @Test func aCooldownIsRoundedUpAndNeverReadsAsZero() {
        #expect(MetadataFetchFlow.humanWait(0) == "1s")
        #expect(MetadataFetchFlow.humanWait(45) == "45s")
        #expect(MetadataFetchFlow.humanWait(91) == "2m")
        #expect(MetadataFetchFlow.humanWait(600) == "10m")
    }
}

// MARK: - Reading a candidate

struct MetadataFetchCandidateTests {
    @Test func imprintLineDropsThePartsTheProviderLeftEmpty() {
        var candidate = edition()
        candidate.year = nil
        candidate.publisher = "   "
        #expect(MetadataFetchFlow.imprintLine(candidate) == "9780441013593")
    }

    @Test func imprintLineFallsBackToADashWhenNoPrintingIsNamed() {
        // A Hardcover search hit describes a work: no year, no publisher, and
        // no edition ISBN. The row is still worth showing.
        var candidate = edition(isbn13: nil, source: .hardcover)
        candidate.year = nil
        candidate.publisher = nil
        #expect(MetadataFetchFlow.imprintLine(candidate) == MetadataFetchFlow.empty)
    }

    @Test func aBlankOrRelativeCoverURLIsTreatedAsAbsent() {
        var candidate = edition()
        candidate.coverURL = "   "
        #expect(MetadataFetchFlow.coverURL(candidate) == nil)
        // Provider covers are absolute; anything server-relative would need
        // the bearer token this loader doesn't send.
        candidate.coverURL = "/api/covers/abc"
        #expect(MetadataFetchFlow.coverURL(candidate) == nil)
        candidate.coverURL = "  https://covers.example/1.jpg  "
        #expect(MetadataFetchFlow.coverURL(candidate) == "https://covers.example/1.jpg")
    }

    @Test func authorsLineFallsBackToADashWhenTheProviderNamedNone() {
        #expect(MetadataFetchFlow.authorsLine(edition(authors: [])) == MetadataFetchFlow.empty)
    }

    @Test func theTakeAllLabelCountsAndTheStagedNoteAttributes() {
        #expect(MetadataFetchFlow.takeAllLabel(changes: 0) == nil)
        #expect(MetadataFetchFlow.takeAllLabel(changes: 1) == "Take 1 field")
        #expect(MetadataFetchFlow.takeAllLabel(changes: 4) == "Take all 4 fields")
        #expect(MetadataFetchFlow.stagedNote(count: 0, sources: [.hardcover]) == nil)
        // Named, because "4 fields changed" with no attribution is the state a
        // reader can't audit.
        #expect(
            MetadataFetchFlow.stagedNote(count: 2, sources: [.googleBooks])?
                .contains("Google Books") == true
        )
    }

    @Test func theStagedNoteRefusesToNameOneSourceWhenSeveralContributed() {
        // Fields can be taken from several candidates in one session — Back
        // returns to the list without clearing what was staged — so naming the
        // last one touched would attribute the others to a source that never
        // supplied them.
        let note = MetadataFetchFlow.stagedNote(
            count: 3, sources: [.openLibrary, .googleBooks]
        )
        #expect(note?.contains("2 sources") == true)
        #expect(note?.contains("Google Books") == false)
        #expect(note?.contains("Open Library") == false)
    }
}
