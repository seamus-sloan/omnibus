//  ComicArchiveTests.swift
//  The local CBZ page source: page ordering, CRC-checked reads, and the
//  post-download integrity verdict.
//
//  The zip fixtures are built by hand (a port of `db::test_support`'s
//  `build_stored_zip`) rather than through ZIPFoundation, so a test can
//  corrupt a byte and know the recorded CRC no longer matches — and so the
//  ordering cases are byte-for-byte the ones the server's own comic tests
//  pin. Index N here must be the page `/pages/N` would serve.

import Foundation
import Testing

@testable import omnibus

// MARK: - Stored-zip fixture builder

/// CRC-32 (IEEE), the checksum zip records per entry.
private func crc32(_ data: Data) -> UInt32 {
    var crc: UInt32 = 0xFFFF_FFFF
    for byte in data {
        crc ^= UInt32(byte)
        for _ in 0..<8 {
            crc = (crc & 1) != 0 ? (crc >> 1) ^ 0xEDB8_8320 : crc >> 1
        }
    }
    return ~crc
}

/// Minimal stored (uncompressed) zip: local headers, data, central
/// directory, EOCD. Port of `db::test_support::build_stored_zip`.
private func buildStoredZip(_ entries: [(String, Data)]) -> Data {
    var out = Data()
    var central = Data()

    func append<T: FixedWidthInteger>(_ value: T, to data: inout Data) {
        withUnsafeBytes(of: value.littleEndian) { data.append(contentsOf: $0) }
    }

    for (name, bytes) in entries {
        let offset = UInt32(out.count)
        let crc = crc32(bytes)
        let nameBytes = Data(name.utf8)
        let size = UInt32(bytes.count)

        append(UInt32(0x0403_4B50), to: &out)
        append(UInt16(20), to: &out) // version needed
        append(UInt16(0), to: &out) // flags
        append(UInt16(0), to: &out) // method: stored
        append(UInt16(0), to: &out) // mod time
        append(UInt16(0), to: &out) // mod date
        append(crc, to: &out)
        append(size, to: &out) // compressed size
        append(size, to: &out) // uncompressed size
        append(UInt16(nameBytes.count), to: &out)
        append(UInt16(0), to: &out) // extra len
        out.append(nameBytes)
        out.append(bytes)

        append(UInt32(0x0201_4B50), to: &central)
        append(UInt16(20), to: &central) // version made by
        append(UInt16(20), to: &central) // version needed
        append(UInt16(0), to: &central) // flags
        append(UInt16(0), to: &central) // method
        append(UInt16(0), to: &central) // mod time
        append(UInt16(0), to: &central) // mod date
        append(crc, to: &central)
        append(size, to: &central)
        append(size, to: &central)
        append(UInt16(nameBytes.count), to: &central)
        append(UInt16(0), to: &central) // extra
        append(UInt16(0), to: &central) // comment
        append(UInt16(0), to: &central) // disk start
        append(UInt16(0), to: &central) // internal attrs
        append(UInt32(0), to: &central) // external attrs
        append(offset, to: &central)
        central.append(nameBytes)
    }

    let cdOffset = UInt32(out.count)
    out.append(central)
    append(UInt32(0x0605_4B50), to: &out)
    append(UInt16(0), to: &out) // this disk
    append(UInt16(0), to: &out) // cd start disk
    append(UInt16(entries.count), to: &out)
    append(UInt16(entries.count), to: &out)
    append(UInt32(central.count), to: &out)
    append(cdOffset, to: &out)
    append(UInt16(0), to: &out) // comment len
    return out
}

/// Write a fixture archive to a scratch file the test cleans up.
private func writeCBZ(_ entries: [(String, Data)]) throws -> URL {
    let url = FileManager.default.temporaryDirectory
        .appendingPathComponent("comic-test-\(UUID().uuidString).cbz")
    try buildStoredZip(entries).write(to: url)
    return url
}

private let pageOne = Data("PAGE-ONE-BYTES".utf8)
private let pageTwo = Data("PAGE-TWO-BYTES".utf8)

@Suite("Comic archive page listing")
struct ComicArchiveListingTests {
    @Test("pages come out in natural-sort order, junk excluded")
    func pagesAreNaturallyOrderedAndFiltered() throws {
        // Same shape the server's comic tests seed: entry order is not page
        // order, AppleDouble forks carry image extensions but aren't pages,
        // and ComicInfo.xml is metadata, not a page.
        let url = try writeCBZ([
            ("p10.jpg", pageTwo),
            ("p2.jpg", pageOne),
            ("__MACOSX/._p2.jpg", Data("junk".utf8)),
            ("ComicInfo.xml", Data("<ComicInfo/>".utf8)),
        ])
        defer { try? FileManager.default.removeItem(at: url) }

        let archive = try ComicArchive(url: url)
        // Numeric ordering — "p2" before "p10" — is the whole reason the
        // sort is natural rather than lexical.
        #expect(archive.pages == ["p2.jpg", "p10.jpg"])
    }

    @Test("numeric ties order deterministically, like the server")
    func naturalCompareMatchesTheServer() {
        // "007" vs "7": equal numerically, so the run length breaks the tie.
        #expect(ComicArchive.naturalCompare("p7.jpg", "p007.jpg") == .orderedAscending)
        #expect(ComicArchive.naturalCompare("P2.jpg", "p10.jpg") == .orderedAscending)
        #expect(ComicArchive.naturalCompare("b1.png", "a2.png") == .orderedDescending)
        #expect(ComicArchive.naturalCompare("same.png", "same.png") == .orderedSame)
    }

    @Test("a file that is not a zip refuses to open")
    func garbageRefusesToOpen() throws {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("not-a-zip-\(UUID().uuidString).cbz")
        try Data("not a zip at all".utf8).write(to: url)
        defer { try? FileManager.default.removeItem(at: url) }

        #expect(throws: ComicArchiveError.self) {
            _ = try ComicArchive(url: url)
        }
    }
}

@Suite("Comic archive page reads")
struct ComicArchiveReadTests {
    @Test("a page reads back its exact bytes by natural-sort index")
    func pageReadsByIndex() throws {
        let url = try writeCBZ([("p2.png", pageTwo), ("p1.jpg", pageOne)])
        defer { try? FileManager.default.removeItem(at: url) }

        let archive = try ComicArchive(url: url)
        #expect(try archive.page(at: 0) == pageOne)
        #expect(try archive.page(at: 1) == pageTwo)
    }

    @Test("an out-of-range index is an error, not a crash")
    func outOfRangeThrows() throws {
        let url = try writeCBZ([("p1.jpg", pageOne)])
        defer { try? FileManager.default.removeItem(at: url) }

        let archive = try ComicArchive(url: url)
        #expect(throws: ComicArchiveError.self) {
            _ = try archive.page(at: 1)
        }
    }

    @Test("a corrupted page fails its CRC on read")
    func corruptionFailsTheRead() throws {
        var bytes = buildStoredZip([("p1.jpg", pageOne)])
        // Flip one byte inside the page data — structure intact, CRC not.
        let range = bytes.range(of: pageOne)!
        bytes[range.lowerBound] ^= 0xFF
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("comic-corrupt-\(UUID().uuidString).cbz")
        try bytes.write(to: url)
        defer { try? FileManager.default.removeItem(at: url) }

        let archive = try ComicArchive(url: url)
        #expect(throws: Error.self) {
            _ = try archive.page(at: 0)
        }
    }
}

@Suite("Comic archive integrity verdict")
struct ComicArchiveVerifyTests {
    @Test("a clean archive verifies intact")
    func cleanArchiveIsIntact() throws {
        let url = try writeCBZ([("p1.jpg", pageOne), ("p2.png", pageTwo)])
        defer { try? FileManager.default.removeItem(at: url) }
        #expect(ComicArchive.verify(url: url))
    }

    @Test("a flipped byte is provably damage — structure alone would pass it")
    func corruptionIsDamage() throws {
        var bytes = buildStoredZip([("p1.jpg", pageOne)])
        let range = bytes.range(of: pageOne)!
        bytes[range.lowerBound] ^= 0xFF
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("comic-damaged-\(UUID().uuidString).cbz")
        try bytes.write(to: url)
        defer { try? FileManager.default.removeItem(at: url) }

        #expect(!ComicArchive.verify(url: url))
    }

    @Test("an archive with no pages is a broken download, not an empty book")
    func pagelessArchiveIsDamage() throws {
        let url = try writeCBZ([("ComicInfo.xml", Data("<ComicInfo/>".utf8))])
        defer { try? FileManager.default.removeItem(at: url) }
        #expect(!ComicArchive.verify(url: url))
    }

    @Test("a file that is not an archive at all is damage")
    func garbageIsDamage() throws {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("garbage-\(UUID().uuidString).cbz")
        try Data("garbage".utf8).write(to: url)
        defer { try? FileManager.default.removeItem(at: url) }
        #expect(!ComicArchive.verify(url: url))
    }
}
