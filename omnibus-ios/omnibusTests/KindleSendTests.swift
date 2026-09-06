//  KindleSendTests.swift
//  What the Send-to-Kindle row decides before it ever reaches the network,
//  and how the poll reads the worker's answer off the wire.
//
//  The gate is the load-bearing part: it is what keeps a command off the
//  outbox (rule 08 test 2) by refusing to offer itself offline, and what stops
//  a doomed send — no address, a file over Kindle's email cap — from being
//  enqueued only to fail on the server minutes later.

import Foundation
import Testing

@testable import omnibus

@Suite("Send to Kindle")
struct KindleSendTests {
    /// The state a book that can actually be sent arrives in.
    private func gate(
        hasEpub: Bool = true,
        size: Int64? = 1_000_000,
        email: String? = "reader@kindle.com",
        online: Bool = true
    ) -> KindleGate {
        KindleService.gate(
            hasEpub: hasEpub, epubSizeBytes: size, kindleEmail: email, isOnline: online
        )
    }

    // MARK: - The gate

    @Test("offers the action for an online reader with an address and a sendable EPUB")
    func readyWhenEverythingIsInPlace() {
        #expect(gate() == .ready)
    }

    @Test("hides the action for a book with no EPUB — there is nothing to convert")
    func hiddenWithoutAnEpub() {
        let g = gate(hasEpub: false)
        #expect(g == .noEpub)
        #expect(g.isHidden)
        // Never drawn, so never pressable — the two have to agree, or the
        // tappable-rows invariant below would be vacuously true for it.
        #expect(!g.isTappable)
    }

    @Test("blocks a send with no Kindle address, and names the setting to fix")
    func blockedWithoutAnAddress() {
        #expect(gate(email: nil) == .noAddress)
        // A saved-then-cleared address comes back as an empty string, which is
        // no more sendable than a missing key.
        #expect(gate(email: "") == .noAddress)
        #expect(gate(email: "   ") == .noAddress)
        // It stays tappable so the tap can explain: a greyed row with nothing
        // to say would leave the reader to guess which of the two settings
        // screens they are missing.
        let g = gate(email: nil)
        #expect(g.isTappable)
        #expect(g.blockedReport?.message.contains("Account") == true)
    }

    @Test("disables the action offline rather than queueing the send")
    func blockedOffline() {
        // Rule 08 test 2: a send is a command, so it is refused up front
        // rather than deferred to the outbox and replayed against a book the
        // reader may have finished by then.
        let g = gate(online: false)
        #expect(g == .offline)
        #expect(!g.isTappable)
    }

    @Test("routes an EPUB over the email cap to Amazon's uploader instead of refusing")
    func oversizeStaysActionable() {
        let g = gate(size: KindleService.maxEmailBytes + 1)
        #expect(g == .oversize)
        // Blocked from emailing, but still worth tapping — it opens the web
        // uploader, which takes files this size.
        #expect(g.isTappable)
        #expect(!g.isHidden)
    }

    @Test("treats a file exactly at the cap as sendable, matching the server's check")
    func sizeCapIsInclusive() {
        #expect(gate(size: KindleService.maxEmailBytes) == .ready)
    }

    @Test("sends a book whose size the listing didn't carry, leaving the cap to the server")
    func unknownSizeFallsThrough() {
        #expect(gate(size: nil) == .ready)
    }

    @Test("reads a negative size as unknown rather than as an enormous file")
    func negativeSizeIsNotOversize() {
        // Signed arithmetic, so the unsigned wrap the web button has to guard
        // against can't happen here — pinned so a later `UInt64` port can't
        // reintroduce it.
        #expect(gate(size: -1) == .ready)
    }

    @Test("prefers the reason that outlasts a reconnect when several apply")
    func reasonsAreOrderedByWhatSurvivesFixing() {
        // Offline *and* over the cap: reconnecting won't shrink the file, so
        // the row points at the uploader rather than saying "you're offline".
        #expect(gate(size: KindleService.maxEmailBytes + 1, online: false) == .oversize)
        // Offline *and* no address: the address is still missing once the
        // network is back.
        #expect(gate(email: nil, online: false) == .noAddress)
    }

    @Test("gives every blocked case a subtitle, and a sendable one none")
    func onlyBlockedCasesExplainThemselves() {
        #expect(KindleGate.ready.reason == nil)
        #expect(KindleGate.noEpub.reason == nil)
        for blocked in [KindleGate.oversize, .noAddress, .offline] {
            #expect(blocked.reason?.isEmpty == false)
        }
    }

    @Test("never leaves a live row with nothing to do when tapped")
    func everyTappableRowHasAnAnswer() {
        // A row the reader can press must either send, go somewhere, or say
        // why not. `.ready` sends and `.oversize` opens the web uploader, so
        // every *other* tappable case owes a report.
        for g in [KindleGate.ready, .noEpub, .oversize, .noAddress, .offline]
        where g.isTappable && g != .ready && g != .oversize {
            #expect(g.blockedReport != nil)
        }
        // And the two that act instead of explaining don't also raise an
        // alert over the top of what they just did.
        #expect(KindleGate.ready.blockedReport == nil)
        #expect(KindleGate.oversize.blockedReport == nil)
    }

    // MARK: - The status poll

    private func decodeStatus(_ json: String) throws -> KindleSendStatus {
        try JSONDecoder().decode(KindleSendStatus.self, from: Data(json.utf8))
    }

    @Test("decodes the worker's tagged status the way the server spells it")
    func decodesEachStatus() throws {
        #expect(try decodeStatus(#"{"status":"pending"}"#) == .pending)
        #expect(try decodeStatus(#"{"status":"sent"}"#) == .sent)
        #expect(
            try decodeStatus(#"{"status":"failed","message":"no SMTP relay"}"#)
                == .failed("no SMTP relay")
        )
    }

    @Test("refuses a status it doesn't recognize instead of reading it as success")
    func rejectsAnUnknownStatus() {
        #expect(throws: (any Error).self) {
            try decodeStatus(#"{"status":"queued"}"#)
        }
    }

    // MARK: - Failure copy

    @Test("carries the server's own sentence when it has one")
    func rejectionKeepsTheServerMessage() {
        #expect(
            KindleSendError.rejected("this book has no EPUB file to send").errorDescription
                == "this book has no EPUB file to send"
        )
    }

    @Test("still says something when the failure arrived without a message")
    func rejectionWithoutAMessageStillReads() {
        // The corollary to rule 08: a refused command must fail visibly. An
        // empty message would render as a blank alert, which reads as silence.
        #expect(KindleSendError.rejected("").errorDescription?.isEmpty == false)
    }

    @Test("does not report an unconfirmed send as delivered")
    func unconfirmedDoesNotClaimDelivery() {
        let message = KindleSendError.unconfirmed.errorDescription ?? ""
        #expect(!message.isEmpty)
        #expect(message.lowercased().contains("confirm"))
    }
}
