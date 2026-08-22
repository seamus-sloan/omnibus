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

    @Test func fieldOrderIsStableAcrossEncodes() {
        // Dictionary iteration order varies per process; the fields array is
        // what keeps a request body — and this test — deterministic.
        let fields = UploadFlow.commitFields(
            kind: .ebook, title: "Dune", author: "Herbert", series: "Dune", seriesIndex: "1"
        )
        let first = decoded(MultipartBody.encode(boundary: "B", fields: fields, files: []))
        let second = decoded(MultipartBody.encode(boundary: "B", fields: fields, files: []))
        #expect(first == second)
    }

    @Test func boundariesAreUniquePerBody() {
        #expect(MultipartBody.makeBoundary() != MultipartBody.makeBoundary())
        #expect(MultipartBody.makeBoundary().hasPrefix("omnibus."))
    }
}
