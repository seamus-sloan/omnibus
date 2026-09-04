//  DownloadPolicy.swift
//  Whether a download may move bytes right now, and what to tell the reader.
//
//  The background session ran flat out with no regard for what else the link
//  was doing: on 2026-09-03 four parts of a 1.2 GB audiobook resumed one second
//  after launch and took the path's whole ceiling while the player streamed the
//  same book over it, so the listener heard constant rebuffering against an
//  idle server (#2409). The decision is kept here, free of URLSession, so the
//  policy is testable without a live transfer.

import Foundation

/// Why a transfer is or is not moving. Doubles as the Downloads list's state
/// line — a stalled bar that says nothing is indistinguishable from a broken
/// one, which is how the incident above read to the reader.
enum DownloadActivity: Equatable {
    case running
    /// Held back so a stream over the same link keeps its bandwidth.
    case pausedForPlayback
    /// Held back because the only path available is cellular or metered and
    /// the reader asked for Wi-Fi only.
    case waitingForWiFi
    /// Stopped on a transient error; resumes from its byte offset on reconnect.
    case retrying(String)
    case failed(String)
    case complete

    /// What the Downloads list and the book detail print under the bar.
    var label: String {
        switch self {
        case .running: "Downloading"
        case .pausedForPlayback: "Paused while listening"
        case .waitingForWiFi: "Waiting for Wi-Fi"
        case .retrying(let why): "Retrying — \(why)"
        case .failed(let why): why
        case .complete: "Downloaded"
        }
    }

    /// Whether this state should read as stalled rather than progressing, so
    /// the bar can stop implying movement it isn't making.
    var isHalted: Bool {
        switch self {
        case .running, .complete: false
        case .pausedForPlayback, .waitingForWiFi, .retrying, .failed: true
        }
    }
}

/// The transfer-gating rules.
enum DownloadPolicy {
    /// Whether a transfer may move bytes.
    ///
    /// Playback wins over downloading unconditionally, not just for the book
    /// being streamed: the contention is on the phone's uplink, which every
    /// part shares regardless of which book it belongs to. A download resumes
    /// the moment the player stops — nobody is waiting on a background
    /// transfer the way they are waiting on the next sentence of a book.
    static func mayTransfer(isStreaming: Bool, wifiOnly: Bool, pathIsExpensive: Bool) -> Bool {
        if isStreaming { return false }
        if wifiOnly, pathIsExpensive { return false }
        return true
    }

    /// The state to show for a transfer that is not moving, in the order the
    /// reasons actually apply — playback is checked first because it is the
    /// one the reader can resolve by pressing pause.
    static func haltReason(isStreaming: Bool, wifiOnly: Bool, pathIsExpensive: Bool)
        -> DownloadActivity?
    {
        if isStreaming { return .pausedForPlayback }
        if wifiOnly, pathIsExpensive { return .waitingForWiFi }
        return nil
    }
}
