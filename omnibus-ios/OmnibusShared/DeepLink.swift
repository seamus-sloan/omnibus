//  DeepLink.swift
//  The `omnibus://` URLs a widget hands back to the app.
//
//  Shared because both ends have to agree exactly: the extension builds these
//  into `widgetURL`, the app parses them in `onOpenURL`, and neither can see
//  the other's mistake — a tap on a malformed link opens the app at whatever
//  screen it was last on, with nothing logged anywhere.

import Foundation

enum DeepLink: Equatable, Sendable {
    /// Open one book. `format` is what the caller asked for; `nil` means it
    /// had no opinion and the app should decide from the book's own formats.
    /// `fileID` names the audiobook file a position was taken in.
    case book(uuid: String, format: WidgetFormat?, fileID: Int64?)

    /// Registered in `omnibus-ios/Info.plist` under `CFBundleURLTypes`.
    static let scheme = "omnibus"

    private static let bookHost = "book"
    private static let formatQuery = "format"
    private static let fileQuery = "file"

    /// Everything a path segment may carry unescaped. `/` is deliberately not
    /// in it: `URLComponents.path` leaves a separator in a value alone, so a
    /// uuid carrying one would silently become two segments and stop parsing.
    /// Book uuids are UUIDv4s or row ids today, but they arrive from the
    /// server and this is the only place that assumption would be load-bearing.
    private static let segmentAllowed = CharacterSet.urlPathAllowed
        .subtracting(CharacterSet(charactersIn: "/"))

    /// `omnibus://` with no destination — the app, and nothing more specific.
    /// What an empty card's tap lands on.
    static let appRoot = URL(string: "\(scheme)://")

    /// `nil` only if `URLComponents` cannot form a URL from a constant scheme,
    /// a constant host and a percent-encoded path — which it cannot. Optional
    /// anyway rather than force-unwrapped: the two consumers are `widgetURL`,
    /// which already takes an optional, and a `Link`, which is inside an `if
    /// let` for exactly this reason. A `!` here would be the one construct in
    /// this file that can crash the widget process.
    var url: URL? {
        switch self {
        case let .book(uuid, format, fileID):
            var components = URLComponents()
            components.scheme = Self.scheme
            components.host = Self.bookHost
            components.percentEncodedPath =
                "/" + (uuid.addingPercentEncoding(withAllowedCharacters: Self.segmentAllowed) ?? "")
            var items: [URLQueryItem] = []
            if let format {
                items.append(URLQueryItem(name: Self.formatQuery, value: format.rawValue))
            }
            if let fileID {
                items.append(URLQueryItem(name: Self.fileQuery, value: String(fileID)))
            }
            components.queryItems = items.isEmpty ? nil : items
            // The scheme alone still names the app, so a link with no book
            // beats no link at all on a uuid the server handed us.
            return components.url ?? Self.appRoot
        }
    }

    init?(_ url: URL) {
        guard let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
              components.scheme?.lowercased() == Self.scheme,
              components.host?.lowercased() == Self.bookHost
        else { return nil }

        // One path component, and it is the uuid. Split the *encoded* path so a
        // `%2F` inside the uuid stays part of its segment — splitting the
        // decoded one would read that book as two segments and refuse the tap.
        let segments = components.percentEncodedPath
            .split(separator: "/", omittingEmptySubsequences: true)
        guard segments.count == 1,
              let uuid = segments.first.map(String.init)?.removingPercentEncoding,
              !uuid.isEmpty
        else { return nil }

        let items = components.queryItems ?? []
        let rawFormat = items.first { $0.name == Self.formatQuery }?.value
        // An unrecognised format is dropped rather than refused: the app can
        // still resolve one from the book, and refusing would turn a widget
        // built against a newer format into a tap that does nothing.
        let format = rawFormat.flatMap(WidgetFormat.init(rawValue:))
        let fileID = items.first { $0.name == Self.fileQuery }?.value.flatMap(Int64.init)

        self = .book(uuid: uuid, format: format, fileID: fileID)
    }
}
