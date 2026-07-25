//  SyncEngine.swift
//  Drains the mutation outbox against the server.
//
//  Port of `frontend/src/offline/sync.rs` + `outbox/apply.rs`. Every offline
//  write lands in `ops` as a replayable request; this replays them in id order
//  and drops each one that lands (or that fails in a way retrying won't fix).

import Foundation

/// Op kinds. The kind doubles as the coalescing key, so a second position
/// write for the same book replaces the first rather than queueing behind it.
enum OpKind {
    static func progress(_ uuid: String) -> String { "progress:\(uuid)" }
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

    private var draining = false

    /// Queue a mutation for replay. `coalesce` collapses repeated writes of
    /// the same logical value (a reading position, a playback rate); discrete
    /// events (a highlight, a session report) must not coalesce.
    func enqueue(
        kind: String,
        path: String,
        method: String = "POST",
        body: Encodable?,
        coalesce: Bool
    ) async {
        var data: Data?
        if let body {
            data = try? JSONEncoder().encode(AnyEncodable(body))
        }
        await OfflineStore.shared.enqueue(
            kind: kind, path: path, method: method, body: data, coalesce: coalesce
        )
        await MainActor.run { Connectivity.shared.notePendingChanged() }
    }

    /// Replay every queued op. Safe to call repeatedly — a second call while
    /// one is in flight is a no-op.
    func drain() async {
        guard !draining else { return }
        draining = true
        defer { draining = false }

        let ops = await OfflineStore.shared.listOps()
        guard !ops.isEmpty else { return }

        var succeeded: [Int64] = []
        for op in ops {
            do {
                try await replay(op)
                succeeded.append(op.id)
            } catch APIError.unauthorized {
                // Nothing will replay until the user signs in again; keep the
                // queue intact and stop.
                break
            } catch let error as APIError where error.isRecoverableOffline {
                // Still offline — stop and retry on the next reconnect.
                break
            } catch let APIError.http(status, message) where (400..<500).contains(status) {
                // The server rejected this payload; retrying can't help. Drop
                // it so one bad op can't wedge the whole queue.
                await OfflineStore.shared.noteOpFailure(op.id, message: message)
                succeeded.append(op.id)
            } catch {
                break
            }
        }

        await OfflineStore.shared.deleteOps(succeeded)
        await MainActor.run { Connectivity.shared.notePendingChanged() }
    }

    private func replay(_ op: PendingOp) async throws {
        let raw = op.body.map { RawJSON(data: $0) }
        switch op.method {
        case "POST":
            let _: Empty = try await APIClient.shared.post(op.path, body: raw ?? RawJSON.null)
        case "PUT":
            let _: Empty = try await APIClient.shared.put(op.path, body: raw ?? RawJSON.null)
        case "PATCH":
            let _: Empty = try await APIClient.shared.patch(op.path, body: raw ?? RawJSON.null)
        case "DELETE":
            let _: Empty = try await APIClient.shared.delete(op.path)
        default:
            let _: Empty = try await APIClient.shared.post(op.path, body: raw ?? RawJSON.null)
        }
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
        case let value as Bool where CFGetTypeID(value as CFTypeRef) == CFBooleanGetTypeID():
            self = .bool(value)
        case let value as NSNumber:
            if CFGetTypeID(value) == CFBooleanGetTypeID() {
                self = .bool(value.boolValue)
            } else {
                self = .number(value.doubleValue)
            }
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
