//  SyncEngine.swift
//  Drains the mutation outbox against the server.
//
//  Port of `frontend/src/offline/sync.rs` + `outbox/apply.rs`. Every offline
//  write lands in `ops` as a replayable request; this replays them in id order
//  and drops each one that lands (or that fails in a way retrying won't fix).

import Foundation

/// Op kinds. The kind doubles as the coalescing key, so a second position
/// write for the same book replaces the first rather than queueing behind it.
///
/// **What belongs here.** The outbox carries per-user *content state* — never
/// configuration, never commands. A write earns a kind only by passing all
/// four tests in `.claude/rules/08-offline-writes.md`:
///
/// 1. **Content state, not configuration.** Instance config (library paths,
///    API keys, SMTP) and account config (`kindle_email`) both stay out: other
///    actors move underneath a deferred write, and a stale replay of a config
///    row is silent damage rather than a late save.
/// 2. **An assertion, not a command.** It must mean the same thing whenever it
///    lands. A reindex or a send-to-Kindle replayed hours later is a job nobody
///    asked for.
/// 3. **Nameable and complete offline.** The device names the target with no
///    server round trip — a client-minted `client_id`, an existing shelf id —
///    and fills the whole payload itself. This is what excludes shelf creates
///    and the check-in routes.
/// 4. **Truthfully representable in the replica.** Every queued write is
///    applied optimistically, so the client must be able to render the result
///    locally rather than showing a promise.
///
/// A write that fails any of them calls `APIClient` directly and surfaces the
/// error. A new kind added here must also declare its cache keys in
/// `OutboxScope` — an undeclared kind blocks every read.
enum OpKind {
    /// Format-scoped, because the kind *is* the coalescing key: sharing one
    /// between a book's epub and audio positions meant queueing a listening
    /// position deleted the reading position queued behind it.
    static func progress(_ uuid: String, _ format: ProgressFormat) -> String {
        "progress:\(uuid):\(format.rawValue)"
    }
    static func rating(_ uuid: String) -> String { "rating:\(uuid)" }
    static func readStatus(_ uuid: String) -> String { "read_status:\(uuid)" }
    static func playbackRate(_ uuid: String) -> String { "playback_rate:\(uuid)" }
    static let highlight = "highlight"
    static let bookmark = "bookmark"
    static let journal = "journal"
    static let session = "session"
    static let shelfMembership = "shelf_membership"
}

actor SyncEngine {
    static let shared = SyncEngine()

    /// The drain in flight, if any. Held rather than a bare flag so a second
    /// caller *waits* for it instead of returning as though the queue were
    /// already empty — `Cache.revalidationIsSafe` asks whether the outbox has
    /// cleared, and a no-op answer would let a read clobber the very rows the
    /// running drain is pushing.
    private var current: Task<Void, Never>?

    /// How many times an op may fail against a *reachable* server before it is
    /// held aside rather than retried.
    ///
    /// A 4xx is terminal on the first answer, and an offline failure isn't an
    /// attempt at all — this bounds the third case: a server that answers 5xx
    /// for this particular payload every time. Without a bound that op sits at
    /// the head of the queue forever, and because a non-empty queue also holds
    /// off replica revalidation, it freezes every server read in the app.
    /// Drains are triggered by lifecycle events, so six spans a good deal of
    /// real time before anything is given up on.
    private static let maxReplayAttempts: Int64 = 6

    /// Statuses that look like a client error but are really "ask again".
    ///
    /// Everything else in 4xx means the server understood the request and will
    /// refuse it identically forever, so retrying is pointless. These three
    /// don't: a timeout and a rate limit are about *when* the request arrived,
    /// not what it said. Treating them as terminal threw away reading positions
    /// and highlights the moment a self-hosted server sat behind a proxy that
    /// throttles.
    private static let retryableClientStatuses: Set<Int> = [408, 425, 429]

    /// Whether a status means this op will never land, however many times it is
    /// sent. Only these are held aside for the You tab instead of retried.
    static func isTerminalRejection(_ status: Int) -> Bool {
        (400..<500).contains(status) && !retryableClientStatuses.contains(status)
    }

    /// Record a mutation and push it, in that order.
    ///
    /// Queueing first is what makes an offline write durable. The old order —
    /// try the request, queue only if it failed — left a window the length of
    /// the request in which the replica already showed the change and nothing
    /// anywhere recorded it: a suspend or a crash there lost the write outright
    /// and the next revalidation quietly replaced it with the server's older
    /// answer. It also let a write racing ahead of the queue reach the server
    /// before ops it depends on, so a note could arrive before the highlight it
    /// hangs on.
    ///
    /// `coalesce` collapses repeated writes of the same logical value (a
    /// reading position, a playback rate); discrete events (a highlight, a
    /// session report) must not coalesce. Answers whether the write left the
    /// queue, so a caller can decide whether refetching would tell it anything.
    @discardableResult
    func write(
        kind: String,
        path: String,
        method: String = "POST",
        body: Encodable?,
        coalesce: Bool
    ) async -> Bool {
        let id = await enqueue(
            kind: kind, path: path, method: method, body: body, coalesce: coalesce
        )
        await drain()
        return !(await OfflineStore.shared.hasOp(id))
    }

    /// Record a mutation without pushing it, for a caller that emits the same
    /// logical value over and over.
    ///
    /// The position writers are why this exists. The reader emits a position
    /// every few seconds of movement and the player every five of playback,
    /// and routing each through [`write`] spent a round trip on each one —
    /// a dozen a minute for as long as someone was reading, against a
    /// self-hosted box on someone's home network. It bought nothing: the queue
    /// coalesces on `kind`, so every one of those writes but the newest is
    /// discarded before it can be sent anyway.
    ///
    /// The op is durable the moment it is queued, so the only thing a deferred
    /// push risks is its own latency — and every moment that actually matters
    /// for cross-device handoff already forces a drain: closing the book,
    /// backgrounding, opening another one, reconnecting. Between those,
    /// `Connectivity`'s pending-queue timer picks it up.
    func record(
        kind: String,
        path: String,
        method: String = "POST",
        body: Encodable?,
        coalesce: Bool
    ) async {
        await enqueue(kind: kind, path: path, method: method, body: body, coalesce: coalesce)
    }

    /// Queue a mutation for replay without pushing it. Prefer [`write`] —
    /// this is for callers that will drain on their own schedule.
    @discardableResult
    func enqueue(
        kind: String,
        path: String,
        method: String = "POST",
        body: Encodable?,
        coalesce: Bool
    ) async -> Int64 {
        var data: Data?
        if let body {
            data = try? JSONEncoder().encode(AnyEncodable(body))
        }
        let id = await OfflineStore.shared.enqueue(
            kind: kind, path: path, method: method, body: data, coalesce: coalesce
        )
        await MainActor.run { Connectivity.shared.notePendingChanged() }
        return id
    }

    /// Replay every queued op. Safe to call repeatedly — a second call while
    /// one is in flight joins it rather than starting another.
    func drain() async {
        if let current {
            await current.value
            return
        }
        let task = Task {
            await performDrain()
            // Released here rather than after the `await` below, so the slot is
            // free the instant the pass ends. Clearing it in the creator's
            // frame left a window in which a caller that had just queued an op
            // joined an already-finished drain and returned as though it had
            // been pushed — the op then waited for whatever triggered the next
            // drain.
            releaseCurrent()
        }
        current = task
        await task.value
    }

    private func releaseCurrent() { current = nil }

    /// Sweep until nothing more can be retired.
    ///
    /// More than one sweep because a caller that queues while a drain is
    /// running *joins* it rather than starting another, and would otherwise be
    /// told its op had been pushed by a pass that had already read the queue
    /// without it. Each sweep strictly shrinks the queue, so this terminates.
    private func performDrain() async {
        while await sweep() {}
        await MainActor.run { Connectivity.shared.notePendingChanged() }
    }

    /// One pass over the queue. Answers whether anything left it, which is the
    /// only condition under which another pass could find more to do.
    private func sweep() async -> Bool {
        let ops = await OfflineStore.shared.listOps()
        guard !ops.isEmpty else { return false }

        var retired = 0
        // Order *within* a kind is load-bearing: a note PATCH addresses the
        // highlight the create ahead of it makes. Order *across* kinds is not —
        // a queued session report has nothing to do with a shelf edit — so a
        // server that keeps 500ing one op holds back only its own kind now,
        // instead of freezing every write in the app behind it.
        var stalled: Set<String> = []

        for op in ops {
            if stalled.contains(op.kind) { continue }
            do {
                let response = try await replay(op)
                // Retired one at a time, not batched at the end of the pass.
                // iOS suspends the app when its background assertion expires,
                // mid-drain and without warning, and a batch delete that never
                // ran meant replaying every landed op on the next launch — a
                // re-sent DELETE 404s and surfaced as a "couldn't sync" warning
                // for a change that had in fact synced.
                await OfflineStore.shared.deleteOps([op.id])
                await applyResponse(response, for: op)
                retired += 1
            } catch APIError.unauthorized {
                // Nothing will replay until the user signs in again; keep the
                // queue intact and stop.
                return false
            } catch let error as APIError where error.isRecoverableOffline {
                // Still offline — stop and retry on the next reconnect.
                return false
            } catch let APIError.http(status, message)
                where Self.isTerminalRejection(status)
            {
                // The server rejected this payload; retrying can't help. Mark
                // it and move on — marked ops are excluded from the queue, so
                // one bad op can't wedge the rest, and the row survives for the
                // You tab to report. Deleting it here is what used to make a
                // change the server never accepted vanish without a trace while
                // the replica went on showing it.
                let detail = message.isEmpty ? "Rejected (\(status))" : message
                await OfflineStore.shared.noteOpFailure(op.id, message: detail)
                // Anything queued behind it that addresses what it would have
                // created is now unlandable too, and sending it would only
                // collect a 404 and report one refused change several times
                // over. `stalled` can't express this: the marked op leaves the
                // queue, so the next sweep would find the dependants at the
                // head and replay them anyway.
                if let token = Self.clientToken(of: op) {
                    await OfflineStore.shared.failOpsReferencing(
                        token, after: op.id, message: detail
                    )
                }
                retired += 1
            } catch is CancellationError {
                // The caller that started this drain went away. Nothing is
                // known about the op either way, so leave it queued.
                return false
            } catch {
                // The server answered, but not with something this op can be
                // retired on — a 5xx, most likely. Worth retrying, but not
                // forever: count the attempt and give up once it is clearly
                // never going to land.
                await noteRetryableFailure(op, error)
                stalled.insert(op.kind)
            }
        }

        return retired > 0
    }

    /// Fold a replayed op's answer back into the replica.
    ///
    /// Only positions have an answer worth keeping, and it matters twice over:
    /// the server resolves position conflicts, so what comes back may be
    /// another device's position rather than the one just sent, and its
    /// `updated_at` is on the server's clock — which is the clock the reader
    /// compares against when it decides whether to offer a jump. Discarding
    /// this left the replica holding a device-clock timestamp that no server
    /// answer could be judged against.
    private func applyResponse(_ response: Data, for op: PendingOp) async {
        guard op.kind.hasPrefix("progress:"),
              let record = try? JSONDecoder().decode(ProgressRecord.self, from: response)
        else { return }
        await Cache.write(CacheKey.progress(record.bookUUID, record.format), record)
        await UserDataService.noteResumePoint(record)
    }

    /// Record one failed-but-retryable attempt, promoting the op to a held
    /// failure once it has burned through [`maxReplayAttempts`].
    private func noteRetryableFailure(_ op: PendingOp, _ error: Error) async {
        let message: String
        if case let APIError.http(status, detail) = error {
            message = detail.isEmpty ? "Server error (\(status))" : detail
        } else {
            message = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
        let attempts = await OfflineStore.shared.noteOpAttempt(op.id)
        guard attempts >= Self.maxReplayAttempts else { return }
        await OfflineStore.shared.noteOpFailure(op.id, message: message)
    }

    /// The client-minted id a queued create carries, which every later op
    /// against the same entity spells into its path. `nil` for a body that has
    /// none — a position, a rating, a session report — none of which anything
    /// else queues against.
    private static func clientToken(of op: PendingOp) -> String? {
        guard let body = op.body,
              let object = try? JSONSerialization.jsonObject(with: body) as? [String: Any]
        else { return nil }
        return (object["client_id"] as? String)?.nilIfBlank
    }

    /// Re-send one stored request, answering with the raw response body.
    private func replay(_ op: PendingOp) async throws -> Data {
        let raw = op.body.map { RawJSON(data: $0) }
        let method = ["POST", "PUT", "PATCH", "DELETE"].contains(op.method) ? op.method : "POST"
        return try await APIClient.shared.sendReturningData(
            path: op.path,
            method: method,
            body: method == "DELETE" ? nil : (raw ?? RawJSON.null)
        )
    }
}

/// Re-encodes an already-serialized body verbatim, so replay sends exactly the
/// bytes the original write produced.
struct RawJSON: Encodable {
    let data: Data

    static let null = RawJSON(data: Data("null".utf8))

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        let object = try JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed])
        try container.encode(JSONValue(object))
    }
}

/// Minimal `Encodable` bridge over `JSONSerialization` output.
indirect enum JSONValue: Encodable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    init(_ any: Any) {
        switch any {
        case is NSNull: self = .null
        // `JSONSerialization` hands back `NSNumber` for both booleans and
        // numbers, and only the CFBoolean type id tells them apart — so the
        // test has to run against the boxed value, before any bridge to a
        // Swift type. Matching `as Bool` first instead did not narrow
        // anything: `NSNumber(1) as? Bool` succeeds, and re-boxing the
        // resulting `Bool` as a `CFTypeRef` always yields a CFBoolean, so the
        // guard was vacuously true. Every `0` and `1` in a queued body
        // replayed as `false`/`true` — a 1-star rating, a 1.0x playback rate,
        // a journal at 1% — and the server rejected each one as a type error
        // the outbox then marked unretryable.
        case let value as NSNumber where CFGetTypeID(value) == CFBooleanGetTypeID():
            self = .bool(value.boolValue)
        case let value as NSNumber:
            self = .number(value.doubleValue)
        case let value as String: self = .string(value)
        case let value as [Any]: self = .array(value.map(JSONValue.init))
        case let value as [String: Any]: self = .object(value.mapValues(JSONValue.init))
        default: self = .null
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case let .bool(value): try container.encode(value)
        case let .number(value):
            // Keep whole numbers integral so the server's `i64` fields parse.
            if value == value.rounded(), abs(value) < 9e15 {
                try container.encode(Int64(value))
            } else {
                try container.encode(value)
            }
        case let .string(value): try container.encode(value)
        case let .array(value): try container.encode(value)
        case let .object(value): try container.encode(value)
        }
    }
}

/// Type-erasing wrapper so `enqueue` can take any `Encodable`.
struct AnyEncodable: Encodable {
    private let encodeImpl: (Encoder) throws -> Void

    init(_ wrapped: Encodable) {
        encodeImpl = wrapped.encode
    }

    func encode(to encoder: Encoder) throws {
        try encodeImpl(encoder)
    }
}
