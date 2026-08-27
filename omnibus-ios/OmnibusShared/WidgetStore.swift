//  WidgetStore.swift
//  The App Group container both processes address the snapshot through.
//
//  A widget runs in its own process with its own sandbox and cannot read the
//  app's container at all, so the shared group is the precondition for every
//  widget rather than an optimisation. Both sides go through here so the
//  layout — where the file lives, what the art is called — is stated once.

import Foundation

enum WidgetStore {
    /// Must match the `com.apple.security.application-groups` entry on *both*
    /// the app and the extension. A mismatch is silent: the container URL
    /// simply comes back `nil` and every read answers with the empty snapshot.
    static let appGroupID = "group.com.omnibus.mobile"

    private static let snapshotFile = "continue-snapshot.json"
    private static let thumbsFolder = "widget-thumbs"

    static var containerURL: URL? {
        FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroupID)
    }

    static var thumbsDirectory: URL? {
        containerURL?.appendingPathComponent(thumbsFolder, isDirectory: true)
    }

    private static var snapshotURL: URL? {
        containerURL?.appendingPathComponent(snapshotFile)
    }

    // MARK: - Snapshot

    /// The snapshot the app last wrote, or `nil` when there is none to read.
    ///
    /// Never throws: a timeline render has nothing useful to do with a decode
    /// failure, and answering `nil` puts the placeholder on screen — which is
    /// the honest picture of a container holding nothing readable.
    static func load() -> WidgetSnapshot? {
        guard let snapshotURL, let data = try? Data(contentsOf: snapshotURL) else { return nil }
        return try? decode(data)
    }

    /// Replace the snapshot. Atomic, because the extension may be reading it:
    /// a partially-written file decodes as nothing and blanks the widget.
    static func save(_ snapshot: WidgetSnapshot) {
        guard let snapshotURL, let data = try? encode(snapshot) else { return }
        try? data.write(to: snapshotURL, options: .atomic)
    }

    /// The snapshot's wire form, as the two processes actually exchange it.
    ///
    /// Exposed rather than folded into `save`/`load` so the round trip can be
    /// pinned without a container: the app writes and the extension reads, and
    /// a test using a default `JSONEncoder` would not exercise the date
    /// strategy below. Timestamps are whole seconds on both sides — a progress
    /// row's `updated_at` already is one — so encoding is lossless.
    static func encode(_ snapshot: WidgetSnapshot) throws -> Data {
        try encoder.encode(snapshot)
    }

    static func decode(_ data: Data) throws -> WidgetSnapshot {
        try decoder.decode(WidgetSnapshot.self, from: data)
    }

    // MARK: - Which book the card is showing

    /// The book a one-book card last advanced to, by `WidgetBook.id`.
    ///
    /// In the App Group's defaults rather than the snapshot file: the app owns
    /// that file and rewrites it wholesale on every position save, so a cursor
    /// kept there would be erased by the next sync. This is the one piece of
    /// widget state the *extension* writes.
    static func cursor() -> String? {
        groupDefaults?.string(forKey: cursorKey)
    }

    static func setCursor(_ id: String?) {
        guard let defaults = groupDefaults else { return }
        if let id {
            defaults.set(id, forKey: cursorKey)
        } else {
            defaults.removeObject(forKey: cursorKey)
        }
    }

    private static let cursorKey = "continue.cursor"

    private static var groupDefaults: UserDefaults? {
        UserDefaults(suiteName: appGroupID)
    }

    // MARK: - Cover art

    /// The extensions a pre-rendered cover can carry. Covers with alpha are
    /// filed as PNG — some are transparent, and flattening one onto JPEG's
    /// implicit black is how a cover that reads fine in the app becomes a dark
    /// slab on the Home Screen. Everything else is JPEG, which is a fraction
    /// of the size for a photographic cover.
    static let thumbExtensions = ["jpg", "png"]

    /// Where a named cover lives, or `nil` when the name is not one this store
    /// could have written.
    ///
    /// Validated on the *read* path as well as the write path. The name comes
    /// out of a decoded file rather than from a value this process built, and
    /// the extension hands it straight to `UIImage(contentsOfFile:)` — so a
    /// name carrying `..` or a separator would read outside the thumbs
    /// directory. Only the app writes the snapshot today; that is a reason the
    /// guard is cheap, not a reason to omit it.
    static func thumbURL(named name: String) -> URL? {
        guard isValidThumbName(name), let thumbsDirectory else { return nil }
        return thumbsDirectory.appendingPathComponent(name)
    }

    static func isValidThumbName(_ name: String) -> Bool {
        !name.isEmpty && !name.contains("/") && !name.hasPrefix(".")
    }

    /// The name a book's cover is filed under. One per book rather than per
    /// (book, format): a dual-format book is two cards showing one artwork.
    static func thumbName(for bookUUID: String, ext: String) -> String {
        // The uuid is a UUIDv4 or a row id, so it is already path-safe — but
        // it arrives from the server, and a separator smuggled into one would
        // write outside the container.
        "\(bookUUID.replacingOccurrences(of: "/", with: "_")).\(ext)"
    }

    /// The name this book's art is *already* filed under, whichever extension
    /// that turned out to be. What lets a refresh that could not re-resolve a
    /// cover keep the one the widget is currently drawing.
    static func existingThumbName(for bookUUID: String) -> String? {
        thumbExtensions
            .map { thumbName(for: bookUUID, ext: $0) }
            .first { name in
                guard let url = thumbURL(named: name) else { return false }
                return FileManager.default.fileExists(atPath: url.path)
            }
    }

    static func thumbModified(named name: String) -> Date? {
        guard let url = thumbURL(named: name) else { return nil }
        return try? url.resourceValues(forKeys: [.contentModificationDateKey])
            .contentModificationDate
    }

    static func writeThumb(_ data: Data, named name: String) {
        guard let url = thumbURL(named: name), let thumbsDirectory else { return }
        try? FileManager.default.createDirectory(
            at: thumbsDirectory, withIntermediateDirectories: true
        )
        try? data.write(to: url, options: .atomic)
    }

    /// Drop art for books that have left the snapshot. Without it the group
    /// container grows by one image per book ever surfaced and never shrinks.
    static func pruneThumbs(keeping names: Set<String>) {
        guard let thumbsDirectory,
              let contents = try? FileManager.default.contentsOfDirectory(
                  at: thumbsDirectory, includingPropertiesForKeys: nil
              )
        else { return }
        for url in contents where !names.contains(url.lastPathComponent) {
            try? FileManager.default.removeItem(at: url)
        }
    }

    // MARK: - Coders

    private static let encoder: JSONEncoder = {
        let e = JSONEncoder()
        e.dateEncodingStrategy = .secondsSince1970
        return e
    }()

    private static let decoder: JSONDecoder = {
        let d = JSONDecoder()
        d.dateDecodingStrategy = .secondsSince1970
        return d
    }()
}
