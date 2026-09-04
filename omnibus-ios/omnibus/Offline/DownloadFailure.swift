//  DownloadFailure.swift
//  Whether a failed transfer is worth resuming or is genuinely over.
//
//  `DownloadManager` routed every error but `.cancelled` to `abandon`, which
//  cancels the sibling parts of a multi-part audiobook, deletes their staged
//  bytes and zeroes the record — so one dropped connection 90% of the way
//  through a 1.2 GB book threw away the whole thing and started again from
//  nothing (#2410). Kept apart from the manager, and free of the URLSession
//  machinery, so the classification is testable on its own.

import Foundation

/// What to do about a transfer that stopped.
enum DownloadFailure: Equatable {
    /// The link went away. The bytes on disk are still good, the server still
    /// has the file, and a `Range` request picks up where this left off.
    case transient
    /// The server gave a definite answer, or the bytes are not what they
    /// claimed to be. Retrying cannot change it.
    case terminal

    /// Classify a `URLSession` task error.
    ///
    /// Everything not positively known to be transient is terminal. The
    /// inverse default would be worse than the bug it replaces: a genuinely
    /// dead download that keeps resuming forever never surfaces an error, so
    /// the reader waits on a book that is never coming.
    static func classify(_ error: Error) -> DownloadFailure {
        guard let url = error as? URLError else { return .terminal }
        switch url.code {
        case .networkConnectionLost,
             .timedOut,
             .notConnectedToInternet,
             .cannotConnectToHost,
             .cannotFindHost,
             .dnsLookupFailed,
             .internationalRoamingOff,
             .callIsActive,
             .dataNotAllowed,
             .secureConnectionFailed:
            return .transient
        default:
            return .terminal
        }
    }

    /// Classify an HTTP status a part came back with. `nil` for a success,
    /// which is not a failure to classify.
    ///
    /// 5xx is the server having a bad moment — a restart mid-transfer is the
    /// common one on a self-hosted box — and 408/429 are explicit invitations
    /// to come back. Any other 4xx means this file is not going to be served
    /// to this device, however many times it asks.
    static func classify(status: Int) -> DownloadFailure? {
        switch status {
        case 200..<300: return nil
        case 408, 429, 500...599: return .transient
        default: return .terminal
        }
    }
}
