//  ComicArchive.swift
//  Local CBZ page reads for a downloaded comic.
//
//  Mirror of the server's `db::comic` reader: the page list, its ordering,
//  and the hidden-file rules must match exactly, because a saved
//  `comic-page:N` anchor is an index into that order — a device that sorted
//  differently would resume on the wrong page. Every read compares the
//  entry's recorded CRC-32 against the checksum ZIPFoundation computes on
//  extract (the comparison is ours — see `page(at:)`), which is what makes
//  a downloaded CBZ part of the CRC-backed tier of integrity checking.

import Foundation
import ZIPFoundation

enum ComicArchiveError: LocalizedError {
    case unreadable(String)
    case pageOutOfRange(Int)
    case pageTooLarge(String)
    case pageDamaged(String)

    var errorDescription: String? {
        switch self {
        case let .unreadable(detail): "This comic couldn't be opened. \(detail)"
        case let .pageOutOfRange(index): "Page \(index + 1) doesn't exist in this comic."
        case let .pageTooLarge(name): "Page \(name) is too large to display."
        case let .pageDamaged(name): "Page \(name) is damaged and couldn't be read."
        }
    }
}

final class ComicArchive {
    /// Image-entry extensions that count as comic pages — the same set the
    /// server's scanner and page endpoint admit.
    static let pageExtensions: Set<String> = ["png", "jpg", "jpeg", "webp", "gif"]

    /// Hard cap on a single decompressed page, mirroring the server's
    /// zip-bomb bound: a crafted entry can claim a false size or lean on
    /// deflate's worst-case ratio, so the consumer counts real bytes.
    static let maxPageBytes = 64 * 1024 * 1024

    /// The archive's page entry names in natural-sort reading order. The
    /// list's length is the page count; index N is the page the server's
    /// `/pages/N` would serve.
    let pages: [String]

    private let archive: Archive

    init(url: URL) throws {
        do {
            archive = try Archive(url: url, accessMode: .read)
        } catch {
            throw ComicArchiveError.unreadable(String(describing: error))
        }
        pages = Self.pageNames(in: archive)
    }

    /// Read page `index` (0-based, in [`pages`] order), CRC-checked as it
    /// decompresses. Bounded by [`maxPageBytes`] so a hostile archive can't
    /// balloon in memory.
    ///
    /// The CRC comparison is explicit: ZIPFoundation's `extract` *computes*
    /// the checksum but leaves comparing it against the entry's recorded one
    /// to the caller (only its whole-directory `unzipItem` compares).
    func page(at index: Int) throws -> Data {
        guard pages.indices.contains(index) else {
            throw ComicArchiveError.pageOutOfRange(index)
        }
        let name = pages[index]
        guard let entry = archive[name] else {
            throw ComicArchiveError.unreadable("missing entry \(name)")
        }
        var bytes = Data()
        let checksum = try archive.extract(entry, skipCRC32: false) { chunk in
            guard bytes.count + chunk.count <= Self.maxPageBytes else {
                throw ComicArchiveError.pageTooLarge(name)
            }
            bytes.append(chunk)
        }
        guard checksum == entry.checksum else {
            throw ComicArchiveError.pageDamaged(name)
        }
        return bytes
    }

    /// Whether every page entry reads clean against its recorded CRC-32 —
    /// the post-download integrity check. Reading each to EOF is what
    /// performs the comparison; structure alone would pass a same-length
    /// splice whose offsets all still resolve.
    ///
    /// Pages only, under the same per-entry decompression bound as a read:
    /// the verdict is about whether the comic can be *read* offline, and
    /// extracting arbitrary non-page entries unbounded would hand a crafted
    /// archive a zip-bomb lever in the very check meant to reject it.
    static func verify(url: URL) -> Bool {
        guard let archive = try? Archive(url: url, accessMode: .read) else { return false }
        let pages = pageNames(in: archive)
        // An archive with nothing to show is a broken download, not an
        // intact empty book — the scanner refuses to index one either.
        guard !pages.isEmpty else { return false }
        for name in pages {
            guard let entry = archive[name], entryReadsClean(archive, entry) else {
                return false
            }
        }
        return true
    }

    /// Extract `entry` to EOF under [`maxPageBytes`] and answer whether its
    /// bytes match the recorded CRC-32. The comparison is ours — extract
    /// only computes the checksum, it never judges it.
    private static func entryReadsClean(_ archive: Archive, _ entry: Entry) -> Bool {
        var seen = 0
        let checksum = try? archive.extract(entry, skipCRC32: false) { chunk in
            seen += chunk.count
            guard seen <= maxPageBytes else {
                throw ComicArchiveError.pageTooLarge(entry.path)
            }
        }
        return checksum == entry.checksum
    }

    /// List the archive's page entries the way the server does: image
    /// extensions only, no directories, no hidden segments (macOS
    /// AppleDouble junk like `__MACOSX/._p001.jpg` carries an image
    /// extension but isn't a page), natural-sorted.
    private static func pageNames(in archive: Archive) -> [String] {
        var names: [String] = []
        for entry in archive where entry.type == .file {
            let path = entry.path
            guard !isHiddenName(path), isPageName(path) else { continue }
            names.append(path)
        }
        names.sort { naturalCompare($0, $1) == .orderedAscending }
        return names
    }

    /// `true` when any path segment starts with `.`.
    static func isHiddenName(_ name: String) -> Bool {
        name.split(separator: "/", omittingEmptySubsequences: false)
            .contains { $0.hasPrefix(".") }
    }

    static func isPageName(_ name: String) -> Bool {
        let ext = (name as NSString).pathExtension.lowercased()
        return pageExtensions.contains(ext)
    }

    /// Natural-sort comparison for page order, ported from the server's
    /// `natural_cmp`: digit runs compare numerically ("p2" < "p10"),
    /// everything else compares case-insensitively, and numeric ties fall
    /// back to run length so "007" and "7" order deterministically.
    static func naturalCompare(_ a: String, _ b: String) -> ComparisonResult {
        var ac = Array(a)[...]
        var bc = Array(b)[...]
        while true {
            switch (ac.first, bc.first) {
            case (nil, nil): return .orderedSame
            case (nil, _): return .orderedAscending
            case (_, nil): return .orderedDescending
            case let (x?, y?) where x.isASCII && x.isNumber && y.isASCII && y.isNumber:
                let da = takeDigits(&ac)
                let db = takeDigits(&bc)
                // Compare stripped of leading zeros: more digits wins, then
                // lexical (same length ⇒ digit-wise is numeric ordering).
                let sa = String(da.drop { $0 == "0" })
                let sb = String(db.drop { $0 == "0" })
                if sa.count != sb.count {
                    return sa.count < sb.count ? .orderedAscending : .orderedDescending
                }
                if sa != sb {
                    return sa < sb ? .orderedAscending : .orderedDescending
                }
                if da.count != db.count {
                    return da.count < db.count ? .orderedAscending : .orderedDescending
                }
            case let (x?, y?):
                let lx = String(x).lowercased()
                let ly = String(y).lowercased()
                if lx != ly {
                    return lx < ly ? .orderedAscending : .orderedDescending
                }
                ac = ac.dropFirst()
                bc = bc.dropFirst()
            }
        }
    }

    private static func takeDigits(_ chars: inout ArraySlice<Character>) -> [Character] {
        var digits: [Character] = []
        while let c = chars.first, c.isASCII, c.isNumber {
            digits.append(c)
            chars = chars.dropFirst()
        }
        return digits
    }
}
