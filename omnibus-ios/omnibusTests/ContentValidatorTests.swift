//  ContentValidatorTests.swift
//  How a download decides its copy has been superseded.
//
//  The wire shape and the per-kind pick are what these pin. Both have a
//  failure mode no screen makes obvious: a validator that never decodes leaves
//  every download looking current forever, and reading the wrong field reports
//  a permanent, unclearable "update available" on every dual-format book.

import Foundation
import Testing

@testable import omnibus

private func book(epub: String?, audio: String?) -> Book {
    var book = Book(id: 1, filename: "b.epub")
    book.uniqueIdentifier = "book-uuid"
    book.epubValidator = epub
    book.audioValidator = audio
    return book
}

@Test func sourceValidatorReadsThePerKindFieldTheServerResolved() {
    let subject = book(epub: "\"epub-1\"", audio: "\"audio-1\"")

    #expect(subject.sourceValidator(for: .ebook) == "\"epub-1\"")
    #expect(subject.sourceValidator(for: .audio) == "\"audio-1\"")
}

@Test func sourceValidatorIsNilWhenTheServerReportedNoneForThatKind() {
    let subject = book(epub: "\"epub-1\"", audio: nil)

    #expect(subject.sourceValidator(for: .audio) == nil)
}

@Test func bookDecodesTheHoistedValidatorsFromTheWire() throws {
    // These ride on the book payload rather than inside `book_files`, which is
    // only sent for multi-file books — the ordinary single-file book would
    // otherwise have nothing to compare against.
    let json = """
        {"id":1,"filename":"a.epub","epub_validator":"\\"5f5e100-abc\\"",
         "audio_validator":"\\"600-def\\""}
        """
    let decoded = try JSONDecoder().decode(Book.self, from: Data(json.utf8))

    #expect(decoded.epubValidator == "\"5f5e100-abc\"")
    #expect(decoded.audioValidator == "\"600-def\"")
}

@Test func bookToleratesAServerThatReportsNoValidators() throws {
    let json = """
        {"id":1,"filename":"a.epub"}
        """
    let decoded = try JSONDecoder().decode(Book.self, from: Data(json.utf8))

    #expect(decoded.epubValidator == nil)
    #expect(decoded.sourceValidator(for: .ebook) == nil)
}

@Test func bookFileInfoStillCarriesItsOwnValidatorForAnExplicitFilePick() throws {
    let json = """
        {"id":7,"format":"EPUB","filename":"a","ordinal":1,"size_bytes":10,"validator":"\\"row-7\\""}
        """
    let decoded = try JSONDecoder().decode(BookFileInfo.self, from: Data(json.utf8))

    #expect(decoded.validator == "\"row-7\"")
}
