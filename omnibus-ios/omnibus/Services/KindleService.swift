//  KindleService.swift
//  Send-to-Kindle: the gate deciding whether the action can run at all, and
//  the enqueue-then-poll command behind it.
//
//  A command, not content state, so per rule 08 test 2 it calls `APIClient`
//  directly and is never handed to `SyncEngine`: a send queued on a plane
//  delivers a book the reader has since finished. The control is disabled
//  while offline instead, and every failure is surfaced rather than swallowed.
//
//  Mirrors the web button in `frontend/src/components/format_switcher/
//  kindle.rs` — same enqueue-then-poll shape, same oversize escape hatch.

import Foundation

/// Terminal-or-pending state of an enqueued send.
///
/// Mirrors `omnibus_shared::KindleSendStatus`, whose `#[serde(tag = "status",
/// rename_all = "snake_case")]` puts the variant name in a `status` field
/// beside `failed`'s `message`.
enum KindleSendStatus: Decodable, Equatable, Sendable {
    case pending
    case sent
    case failed(String)

    private enum CodingKeys: String, CodingKey {
        case status
        case message
    }

    init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let status = try container.decode(String.self, forKey: .status)
        switch status {
        case "pending": self = .pending
        case "sent": self = .sent
        case "failed":
            self = .failed(try container.decodeIfPresent(String.self, forKey: .message) ?? "")
        default:
            throw DecodingError.dataCorruptedError(
                forKey: .status, in: container,
                debugDescription: "unknown send status \"\(status)\""
            )
        }
    }
}

/// Whether the Send-to-Kindle action can run, and why not when it can't.
///
/// Everything but `.ready` and `.noEpub` still renders a row: a reader who
/// finds nothing in the menu learns nothing, where a disabled row carrying its
/// own reason says what to fix.
enum KindleGate: Equatable {
    case ready
    /// Nothing to convert — the endpoint sends the EPUB or errors `NoEpub`.
    case noEpub
    /// Over Kindle's email cap. Still actionable: Amazon's own uploader takes
    /// it, so the row opens that rather than being a dead end.
    case oversize
    case noAddress
    case offline

    /// Whether the action is absent from the menu entirely.
    var isHidden: Bool { self == .noEpub }

    /// Whether tapping the row does something.
    ///
    /// Two blocked cases stay live, because a greyed row is only an honest
    /// answer when the reader can already tell why. `.oversize` has somewhere
    /// else to send them, and `.noAddress` has something to say. `.offline` is
    /// the one that doesn't need either: the app says so globally, and every
    /// other network-only control here greys out the same way. `.noEpub` is
    /// never drawn at all, so it is not tappable in any sense.
    var isTappable: Bool {
        switch self {
        case .ready, .oversize, .noAddress: true
        case .offline, .noEpub: false
        }
    }

    /// The row's subtitle — a menu label's second `Text`. Supplementary, never
    /// the only explanation: [`blockedReport`] is what a tap actually shows.
    var reason: String? {
        switch self {
        case .ready, .noEpub: nil
        case .oversize: "Too large to email"
        case .noAddress: "No Kindle address yet"
        case .offline: "You're offline"
        }
    }

    /// What tapping a blocked-but-live row explains, when there is no send to
    /// make and nowhere to send the reader instead.
    var blockedReport: KindleReport? {
        guard self == .noAddress else { return nil }
        return KindleReport(
            title: "No Kindle address yet",
            message: "Set the address your Kindle delivers to on the Account screen, "
                + "then send this book again."
        )
    }
}

/// Why a send didn't complete. Every case carries a reader-facing sentence:
/// rule 08's corollary is that a write this client refuses to queue must fail
/// visibly, never `try?`-swallowed.
enum KindleSendError: LocalizedError, Equatable {
    /// The worker ran the job and it failed; the message is the server's own.
    case rejected(String)
    /// The job never reached a terminal state this client could see — the task
    /// id went unknown, or the poll outlasted its budget.
    case unconfirmed

    var errorDescription: String? {
        switch self {
        case let .rejected(message):
            message.nilIfBlank ?? "The server couldn't deliver this book."
        case .unconfirmed:
            "Couldn't confirm the send finished. Check your Kindle before trying again."
        }
    }
}

/// What a finished send has to say for itself — the alert's payload.
/// `Identifiable` so a second send re-presents rather than reusing the first
/// alert's text.
struct KindleReport: Identifiable, Equatable {
    let id = UUID()
    let title: String
    let message: String
}

enum KindleService {
    /// Kindle's cap on an emailed file, mirroring
    /// `omnibus_shared::KINDLE_EMAIL_MAX_BYTES`.
    static let maxEmailBytes: Int64 = 50_000_000

    /// Amazon's own Send to Kindle page, which accepts files up to 200 MB —
    /// where an EPUB too big to email still has a way through.
    static let webUploadURL = URL(string: "https://www.amazon.com/sendtokindle")

    /// Decide whether the action can run, from state the caller already holds:
    /// the book's formats and EPUB size, the cached `UserSummary`'s Kindle
    /// address, and reachability.
    ///
    /// Deliberately *not* a check that the server has SMTP configured — that
    /// status is admin-only (`GET /api/smtp`), so no reader's client can
    /// pre-flight it. The web button doesn't either; both learn it from the
    /// enqueue's 409 and surface the server's sentence, which is why
    /// [`send`] reports rather than swallows.
    ///
    /// Order is by what survives fixing: a size cap is a property of the book,
    /// a missing address is an account setting, and being offline passes on its
    /// own. So the reason shown is the one that would still be true once the
    /// reader reconnects.
    static func gate(
        hasEpub: Bool,
        epubSizeBytes: Int64?,
        kindleEmail: String?,
        isOnline: Bool
    ) -> KindleGate {
        guard hasEpub else { return .noEpub }
        // Signed comparison, so a negative size (a corrupt row, a sentinel)
        // reads as "not oversize" and falls through to the normal path — the
        // server re-checks the real size before it reads the file in.
        if let epubSizeBytes, epubSizeBytes > maxEmailBytes { return .oversize }
        guard kindleEmail?.nilIfBlank != nil else { return .noAddress }
        guard isOnline else { return .offline }
        return .ready
    }

    /// Enqueue a send of this book's EPUB to the caller's Kindle address and
    /// poll the worker until it reports an outcome. Returns on delivery and
    /// throws otherwise — there is no third answer a caller may ignore.
    ///
    /// The POST is fast by design (the server's pre-checks run inline, the SMTP
    /// delivery goes to the worker), so a refusal it can answer immediately —
    /// no Kindle address, SMTP unconfigured, unknown book — arrives here as an
    /// `APIError.http` carrying the server's own explanation.
    static func send(uuid: String) async throws {
        let accepted: SendAccepted = try await APIClient.shared.post(
            "/api/kindle/send", body: SendRequest(bookUUID: uuid)
        )
        try await awaitOutcome(taskID: accepted.taskID)
    }

    // MARK: - Internals

    /// Body for `POST /api/kindle/send`. `file_id` is left off: the book-detail
    /// action sends the book, and the server picks its lowest-ordinal EPUB the
    /// same way every other endpoint resolves a file.
    private struct SendRequest: Encodable {
        let bookUUID: String

        enum CodingKeys: String, CodingKey {
            case bookUUID = "book_uuid"
        }
    }

    /// `{"task_id": N}` — the handle the status poll is keyed on.
    private struct SendAccepted: Decodable {
        let taskID: UInt64

        enum CodingKeys: String, CodingKey {
            case taskID = "task_id"
        }
    }

    private static let pollInterval: Duration = .milliseconds(700)

    /// How long to keep asking before giving up on an answer.
    ///
    /// Sized off the server's own `SEND_TIMEOUT` (90s in `db/src/kindle.rs`)
    /// with room for the job to reach a worker slot. Past that the send has
    /// failed on its own terms, and the reader is better served by a sentence
    /// than by a spinner that never resolves.
    private static let pollBudget: Duration = .seconds(150)

    private static func awaitOutcome(taskID: UInt64) async throws {
        let deadline = ContinuousClock.now.advanced(by: pollBudget)
        while ContinuousClock.now < deadline {
            try await Task.sleep(for: pollInterval)
            switch try await status(taskID: taskID) {
            case .pending: continue
            case .sent: return
            case let .failed(message): throw KindleSendError.rejected(message)
            }
        }
        throw KindleSendError.unconfirmed
    }

    /// Poll one enqueued send.
    ///
    /// A 404 means the worker no longer knows this task — never posted, or
    /// evicted past its terminal-retention window. That is not a failure, but
    /// it is not confirmed delivery either, so it becomes `.unconfirmed` rather
    /// than being reported as success.
    private static func status(taskID: UInt64) async throws -> KindleSendStatus {
        do {
            return try await APIClient.shared.get(
                "/api/kindle/send/status", query: ["task_id": String(taskID)]
            )
        } catch let APIError.http(status, _) where status == 404 {
            throw KindleSendError.unconfirmed
        }
    }
}
