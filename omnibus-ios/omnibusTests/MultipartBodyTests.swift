//  MultipartBodyTests.swift
//  The bytes an upload actually puts on the wire. The commit handlers parse
//  `title`/`author` out of this body, so "did we send them" is a property of
//  the encoding, not of the call site — and it is what the app got wrong.

import Foundation
import Testing

@testable import omnibus

private func decoded(_ data: Data) -> String {
    String(decoding: data, as: UTF8.self)
}

private func file(_ name: String, _ bytes: String = "PK\u{03}\u{04}") -> MultipartFile {
    MultipartFile(
        fileName: name,
        mimeType: UploadFlow.mimeType(for: name),
        data: Data(bytes.utf8)
    )
}

struct MultipartBodyTests {
    @Test func encodesTextFieldsAheadOfTheFile() {
        let body = decoded(
            MultipartBody.encode(
                boundary: "B",
                fields: UploadFlow.commitFields(
                    kind: .ebook, title: "Dune", author: "Frank Herbert",
                    series: "", seriesIndex: ""
                ),
                files: [file("Dune.epub")]
            )
        )
        #expect(body.contains("--B\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nDune\r\n"))
        #expect(
            body.contains(
                "--B\r\nContent-Disposition: form-data; name=\"author\"\r\n\r\nFrank Herbert\r\n"
            )
        )
        #expect(
            body.contains(
                "Content-Disposition: form-data; name=\"file\"; filename=\"Dune.epub\"\r\n"
                    + "Content-Type: application/epub+zip\r\n\r\n"
            )
        )
        #expect(body.hasSuffix("--B--\r\n"))
        // The regression: a commit body with no title/author part 400s.
        let titleIndex = body.range(of: "name=\"title\"")
        let fileIndex = body.range(of: "name=\"file\"")
        #expect(titleIndex != nil)
        #expect(fileIndex != nil)
        if let titleIndex, let fileIndex {
            #expect(titleIndex.lowerBound < fileIndex.lowerBound)
        }
    }

    @Test func encodesEveryPartOfAMultiFileAudiobookUnderTheSameFieldName() {
        // The audiobook endpoint collects repeated `file` fields into one book.
        let body = decoded(
            MultipartBody.encode(
                boundary: "B",
                fields: [("title", "Dune"), ("author", "Herbert")],
                files: [file("01.mp3", "ID3a"), file("02.mp3", "ID3b")]
            )
        )
        #expect(body.contains("filename=\"01.mp3\""))
        #expect(body.contains("filename=\"02.mp3\""))
        #expect(body.components(separatedBy: "name=\"file\"").count == 3)
        #expect(body.contains("Content-Type: audio/mpeg"))
    }

    @Test func encodesAFileWithNoFieldsForTheAvatarRoute() {
        let body = decoded(
            MultipartBody.encode(
                boundary: "B",
                fields: [],
                files: [
                    MultipartFile(
                        fieldName: "avatar", fileName: "avatar.jpg",
                        mimeType: "image/jpeg", data: Data([0xFF, 0xD8])
                    )
                ]
            )
        )
        #expect(body.contains("name=\"avatar\"; filename=\"avatar.jpg\""))
        #expect(!body.contains("name=\"title\""))
    }

    @Test func preservesBinaryPayloadBytesExactly() {
        let payload = Data([0x50, 0x4B, 0x03, 0x04, 0x00, 0xFF, 0x0D, 0x0A])
        let body = MultipartBody.encode(
            boundary: "B",
            fields: [],
            files: [
                MultipartFile(fileName: "a.epub", mimeType: "application/epub+zip", data: payload)
            ]
        )
        #expect(body.range(of: payload) != nil)
    }

    @Test func fieldsAppearInTheOrderGiven() {
        // Asserts the concrete byte order, not that two encodes of one array
        // agree — which is true of any deterministic encoder, including a
        // Dictionary-based one, so it could not fail.
        let body = decoded(
            MultipartBody.encode(
                boundary: "B",
                fields: UploadFlow.commitFields(
                    kind: .ebook, title: "Dune", author: "Herbert",
                    series: "Dune", seriesIndex: "1"
                ),
                files: []
            )
        )
        let positions = ["title", "author", "series", "series_index"].map {
            body.range(of: "name=\"\($0)\"")?.lowerBound
        }
        #expect(positions.allSatisfy { $0 != nil })
        #expect(positions == positions.sorted { ($0 ?? body.endIndex) < ($1 ?? body.endIndex) })
    }

    @Test func escapesAQuoteThatWouldCloseTheHeaderEarly() {
        // `"` is legal in a POSIX filename; raw, it ends the quoted string and
        // corrupts every header after it.
        let body = decoded(
            MultipartBody.encode(
                boundary: "B", fields: [],
                files: [
                    MultipartFile(
                        fileName: #"He said "hi".epub"#,
                        mimeType: "application/epub+zip", data: Data("PK".utf8)
                    )
                ]
            )
        )
        #expect(body.contains(#"filename="He said %22hi%22.epub""#))
        // Exactly two quotes around the value, so the parameter is still one
        // well-formed quoted string.
        let header = body.components(separatedBy: "\r\n").first { $0.contains("filename=") }
        #expect(header?.filter { $0 == "\"" }.count == 4)
    }

    @Test func escapesNewlinesThatWouldEndTheHeaderLine() {
        let body = decoded(
            MultipartBody.encode(
                boundary: "B", fields: [],
                files: [
                    MultipartFile(
                        fileName: "two\r\nlines.epub",
                        mimeType: "application/epub+zip", data: Data("PK".utf8)
                    )
                ]
            )
        )
        #expect(body.contains(#"filename="two%0D%0Alines.epub""#))
        // The disposition must occupy exactly one physical line. Counting
        // occurrences of "Content-Disposition" cannot show that — splitting one
        // header across three lines does not add occurrences — so count the
        // CRLFs the body is allowed to contain instead: header, blank line,
        // payload terminator, and the closing boundary.
        let dispositionLine = body
            .components(separatedBy: "\r\n")
            .first { $0.contains("Content-Disposition") }
        #expect(dispositionLine?.hasSuffix(#"filename="two%0D%0Alines.epub""#) == true)
        #expect(!body.contains("two\r\nlines"))
    }

    @Test func leavesAPercentAloneSoOrdinaryFilenamesArentMangled() {
        // The server slugifies rather than percent-decoding — a probe upload of
        // `50%25 off.mp3` landed as `50-25-off.mp3` — so pre-encoding `%` would
        // stamp a literal `25` into a book legitimately called "50% off".
        let body = decoded(
            MultipartBody.encode(
                boundary: "B", fields: [],
                files: [
                    MultipartFile(
                        fileName: "50% off.mp3", mimeType: "audio/mpeg", data: Data("ID3".utf8)
                    )
                ]
            )
        )
        #expect(body.contains(#"filename="50% off.mp3""#))
    }

    @Test func escapesTheFieldNameOnTheSameTerms() {
        let body = decoded(
            MultipartBody.encode(
                boundary: "B", fields: [(#"od"d"#, "v")], files: []
            )
        )
        #expect(body.contains(#"name="od%22d""#))
    }

    @Test func boundariesAreUniquePerBody() {
        #expect(MultipartBody.makeBoundary() != MultipartBody.makeBoundary())
        #expect(MultipartBody.makeBoundary().hasPrefix("omnibus."))
    }
}
