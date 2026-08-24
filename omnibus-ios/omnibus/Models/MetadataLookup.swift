//  MetadataLookup.swift
//  Wire types for the metadata editor's provider search — the Swift side of
//  `omnibus_shared::metadata_lookup`.
//
//  Three calls, in the order the picker makes them: `GET /api/metadata/providers`
//  to decide whether to offer a search at all, `POST /api/metadata/editions/search`
//  to fan one out, and `POST /api/metadata/editions/hydrate` to re-fetch the one
//  candidate the reader picked.

import Foundation

/// Which external provider answered — or would be asked.
///
/// A wrapper around the raw string rather than a closed `enum` so a server that
/// grows a fourth provider doesn't fail to decode here: an unknown id still
/// round-trips into a hydrate request and still renders a name, which is what a
/// client that can't be updated in lockstep needs.
struct MetadataProvider: RawRepresentable, Codable, Hashable, Sendable {
    let rawValue: String

    init(rawValue: String) {
        self.rawValue = rawValue
    }

    static let openLibrary = MetadataProvider(rawValue: "open_library")
    static let googleBooks = MetadataProvider(rawValue: "google_books")
    static let hardcover = MetadataProvider(rawValue: "hardcover")

    /// Name for a badge. Falls back to title-casing the id, so an unrecognised
    /// provider reads as "Book Brainz" rather than as `book_brainz`.
    var displayName: String {
        switch rawValue {
        case "open_library": "Open Library"
        case "google_books": "Google Books"
        case "hardcover": "Hardcover"
        default:
            rawValue
                .split(separator: "_")
                .map(\.capitalized)
                .joined(separator: " ")
        }
    }

    /// Tint for the source badge, so two candidates from different sources are
    /// told apart at a glance rather than by reading the label.
    var badgeHue: Double {
        switch rawValue {
        case "open_library": 250
        case "google_books": 155
        case "hardcover": 330
        default: 70
        }
    }
}

/// What one provider can be asked and what it can return, independent of
/// whether it is currently configured.
struct ProviderCapabilities: Codable, Hashable, Sendable {
    var searchByTitle = false
    var searchByIsbn = false
    var carriesCover = false
    var carriesRatings = false
    var carriesGenres = false

    enum CodingKeys: String, CodingKey {
        case searchByTitle = "search_by_title"
        case searchByIsbn = "search_by_isbn"
        case carriesCover = "carries_cover"
        case carriesRatings = "carries_ratings"
        case carriesGenres = "carries_genres"
    }
}

/// One entry of `GET /api/metadata/providers`. Carries no key material —
/// `configured` is a bool, never a masked preview.
struct ProviderInfo: Codable, Hashable, Sendable, Identifiable {
    var id: MetadataProvider
    var displayName: String
    /// Whether this instance would actually invoke the provider right now.
    var configured: Bool
    /// Whether an API key is required to reach it at all.
    var requiresKey: Bool
    var capabilities = ProviderCapabilities()

    enum CodingKeys: String, CodingKey {
        case id, configured, capabilities
        case displayName = "display_name"
        case requiresKey = "requires_key"
    }
}

/// One candidate from one provider, kept attributed and un-collapsed: two
/// printings of a book stay two rows, because telling editions apart is what
/// the picker is for.
struct ProviderEdition: Codable, Hashable, Sendable, Identifiable {
    var source: MetadataProvider
    /// Opaque, provider-scoped handle this candidate is re-fetched by. Never
    /// parsed here — it is echoed back to `hydrate` exactly as it arrived.
    var providerRef: String
    /// Optional deliberately: a search hit is very often a *work* rather than
    /// a printing, and what a candidate must have to be re-fetched is a
    /// handle, not an ISBN.
    var isbn13: String?
    var isbn10: String?
    var title: String
    var authors: [String] = []
    var year: String?
    /// The edition's printed length — what the `print_pages` override stores.
    var pages: Int64?
    var publisher: String?
    var description: String?
    var coverURL: String?
    var series: String?
    var seriesIndex: String?
    var firstPublishYear: Int64?
    var genres: [String] = []
    /// Hundredths of a point, or `nil` from a source that did not score.
    var relevance: Int32?

    enum CodingKeys: String, CodingKey {
        case source, isbn13, isbn10, title, authors, year, pages, publisher
        case description, series, genres, relevance
        case providerRef = "provider_ref"
        case coverURL = "cover_url"
        case seriesIndex = "series_index"
        case firstPublishYear = "first_publish_year"
    }

    /// Stable within one response: a provider never offers the same handle
    /// twice, and two sources' handles are namespaced by `source`.
    var id: String { "\(source.rawValue)#\(providerRef)" }

    var authorDisplay: String {
        authors.isEmpty ? "" : authors.joined(separator: ", ")
    }
}

/// What one provider contributed to a fan-out search.
///
/// The four cases are the point of the type: "it has nothing", "we never asked
/// it", "we asked and never got an answer", and "it rate-limited us recently"
/// are different facts, and collapsing them makes an outage read as a miss.
enum ProviderSearchStatus: Decodable, Hashable, Sendable {
    case answered(count: Int)
    case notConfigured
    case failed(message: String)
    case throttled(retryAfterSecs: UInt64)
    /// A `kind` this build doesn't know. Rendered as a neutral "—" rather than
    /// failing the whole response's decode.
    case unknown

    enum CodingKeys: String, CodingKey {
        case kind, count, message
        case retryAfterSecs = "retry_after_secs"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "answered":
            self = .answered(count: try c.decode(Int.self, forKey: .count))
        case "not_configured":
            self = .notConfigured
        case "failed":
            self = .failed(message: try c.decode(String.self, forKey: .message))
        case "throttled":
            self = .throttled(retryAfterSecs: try c.decode(UInt64.self, forKey: .retryAfterSecs))
        default:
            self = .unknown
        }
    }
}

/// One row of the per-source status table returned alongside the candidates.
struct ProviderSearchSource: Decodable, Hashable, Sendable, Identifiable {
    var provider: MetadataProvider
    var displayName: String
    var status: ProviderSearchStatus

    enum CodingKeys: String, CodingKey {
        case provider, status
        case displayName = "display_name"
    }

    var id: String { provider.rawValue }
}

/// Body for `POST /api/metadata/editions/search`.
///
/// The three structured fields are what let each provider be asked in *its*
/// terms rather than handed one flattened string — Open Library matches
/// `title=` against the title field alone, so "Dune Frank Herbert" as one
/// phrase returns books written *about* Dune.
struct EditionSearchRequest: Encodable, Equatable, Sendable {
    var query: String
    var title: String?
    var author: String?
    var isbn: String?
    /// Which providers to ask. `nil` means every configured one.
    var providers: [MetadataProvider]?
}

/// Fan-out results: the candidates, plus one status row per provider
/// considered — including the ones skipped or failed, which is what lets the
/// sheet say "Hardcover: unavailable" instead of quietly showing a shorter
/// list.
struct EditionSearchResponse: Decodable, Sendable {
    var editions: [ProviderEdition] = []
    var sources: [ProviderSearchSource] = []
}

/// Body for `POST /api/metadata/editions/hydrate` — the second call, naming
/// one edition rather than a query.
struct EditionHydrateRequest: Encodable, Sendable {
    var source: MetadataProvider
    var providerRef: String
    /// Preferred when present, since it names a *printing* where a bare handle
    /// may name a work — but the handle is what makes the re-fetch possible.
    var isbn13: String?

    enum CodingKeys: String, CodingKey {
        case source, isbn13
        case providerRef = "provider_ref"
    }
}

/// Body for `POST /api/ebooks/{uuid}/cover/from-url` — the one field on the
/// compare screen that cannot stage, because applying it means the server
/// fetching the provider's image on the reader's behalf.
struct CoverFromURLRequest: Encodable, Sendable {
    var url: String
}
