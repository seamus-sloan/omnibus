//  DownloadManager.swift
//  Offline file downloads for ebooks and audiobooks.
//
//  Port of `frontend/src/offline/downloads/`. The Rust side pairs the registry
//  with a loopback HTTP server so the WebView can play a local file; on iOS
//  `AVPlayer` and `WKWebView` both load `file://` URLs directly, so the
//  loopback hop is dropped entirely.

import Foundation
import Observation

@Observable
@MainActor
final class DownloadManager: NSObject {
    static let shared = DownloadManager()

    /// Registry mirror, keyed by book uuid, for synchronous view reads.
    private(set) var records: [String: DownloadRecord] = [:]

    private var session: URLSession!
    private var tasks: [Int: String] = [:]

    private override init() {
        super.init()
        let config = URLSessionConfiguration.default
        config.timeoutIntervalForResource = 60 * 60
        session = URLSession(configuration: config, delegate: self, delegateQueue: nil)
    }

    func hydrate() async {
        let all = await OfflineStore.shared.allDownloads()
        var map: [String: DownloadRecord] = [:]
        for var record in all {
            // A download interrupted by a cold kill is not resumable; surface
            // it as failed so the UI offers a retry instead of a stuck bar.
            if record.state == .running || record.state == .queued {
                record.state = .failed
                record.error = "Interrupted"
                await OfflineStore.shared.upsertDownload(record)
            }
            map[record.bookUUID] = record
        }
        records = map
    }

    func record(for uuid: String) -> DownloadRecord? { records[uuid] }

    func isDownloaded(_ uuid: String) -> Bool {
        guard let record = records[uuid], record.state == .complete, let path = record.localPath
        else { return false }
        return FileManager.default.fileExists(atPath: path)
    }

    func localURL(for uuid: String) -> URL? {
        guard let record = records[uuid], record.state == .complete, let path = record.localPath,
              FileManager.default.fileExists(atPath: path)
        else { return nil }
        return URL(fileURLWithPath: path)
    }

    /// Begin (or restart) a download. `format` picks the endpoint: an ebook
    /// pulls `/api/ebooks/{uuid}/file`, an audiobook `/api/audiobooks/{uuid}/download`.
    func start(book: Book) async {
        let uuid = book.uuid
        let isAudio = book.hasAudiobook && !book.hasEbook
        let path = isAudio
            ? "/api/audiobooks/\(uuid)/download"
            : "/api/ebooks/\(uuid)/file"
        let format = isAudio ? (book.formats.first { Book.audioFormats.contains($0.lowercased()) } ?? "m4b")
                             : "epub"

        guard let url = await APIClient.shared.absoluteURL(path) else { return }
        var request = URLRequest(url: url)
        for (key, value) in await APIClient.shared.authHeaders() {
            request.setValue(value, forHTTPHeaderField: key)
        }

        var record = DownloadRecord(
            bookUUID: uuid, format: format, state: .running, localPath: nil,
            totalBytes: 0, receivedBytes: 0, updatedAt: Int64(Date().timeIntervalSince1970),
            error: nil
        )
        records[uuid] = record
        await OfflineStore.shared.upsertDownload(record)

        let task = session.downloadTask(with: request)
        tasks[task.taskIdentifier] = uuid
        task.resume()
        record.state = .running
    }

    func cancel(_ uuid: String) async {
        for (identifier, mapped) in tasks where mapped == uuid {
            let all = await session.allTasks
            all.first { $0.taskIdentifier == identifier }?.cancel()
            tasks[identifier] = nil
        }
        await remove(uuid)
    }

    func remove(_ uuid: String) async {
        await OfflineStore.shared.deleteDownload(uuid)
        records[uuid] = nil
    }

    /// Total bytes held on disk by completed downloads — shown on the You tab.
    func totalBytesOnDisk() -> Int64 {
        records.values.filter { $0.state == .complete }.reduce(0) { $0 + $1.totalBytes }
    }

    fileprivate func update(uuid: String, mutate: (inout DownloadRecord) -> Void) async {
        guard var record = records[uuid] else { return }
        mutate(&record)
        record.updatedAt = Int64(Date().timeIntervalSince1970)
        records[uuid] = record
        await OfflineStore.shared.upsertDownload(record)
    }
}

extension DownloadManager: URLSessionDownloadDelegate {
    nonisolated func urlSession(
        _ session: URLSession,
        downloadTask: URLSessionDownloadTask,
        didWriteData bytesWritten: Int64,
        totalBytesWritten: Int64,
        totalBytesExpectedToWrite: Int64
    ) {
        let identifier = downloadTask.taskIdentifier
        Task { @MainActor in
            guard let uuid = self.tasks[identifier] else { return }
            await self.update(uuid: uuid) { record in
                record.receivedBytes = totalBytesWritten
                if totalBytesExpectedToWrite > 0 { record.totalBytes = totalBytesExpectedToWrite }
            }
        }
    }

    nonisolated func urlSession(
        _ session: URLSession,
        downloadTask: URLSessionDownloadTask,
        didFinishDownloadingTo location: URL
    ) {
        let identifier = downloadTask.taskIdentifier
        let status = (downloadTask.response as? HTTPURLResponse)?.statusCode ?? 0
        // The temp file is deleted the moment this method returns, so the move
        // has to happen synchronously here, not in the Task below.
        let staged = OfflineStore.downloadsDirectory
            .appendingPathComponent("staged-\(identifier)-\(UUID().uuidString)")
        let moved = (try? FileManager.default.moveItem(at: location, to: staged)) != nil

        Task { @MainActor in
            guard let uuid = self.tasks[identifier] else { return }
            self.tasks[identifier] = nil

            guard (200..<300).contains(status), moved else {
                try? FileManager.default.removeItem(at: staged)
                await self.update(uuid: uuid) { record in
                    record.state = .failed
                    record.error = "Download failed (\(status))"
                }
                return
            }

            let format = self.records[uuid]?.format ?? "bin"
            let destination = OfflineStore.downloadsDirectory
                .appendingPathComponent("\(uuid).\(format)")
            try? FileManager.default.removeItem(at: destination)
            do {
                try FileManager.default.moveItem(at: staged, to: destination)
            } catch {
                try? FileManager.default.removeItem(at: staged)
                await self.update(uuid: uuid) { record in
                    record.state = .failed
                    record.error = error.localizedDescription
                }
                return
            }

            let size = (try? FileManager.default.attributesOfItem(atPath: destination.path)[.size] as? Int64) ?? 0
            await self.update(uuid: uuid) { record in
                record.state = .complete
                record.localPath = destination.path
                record.receivedBytes = size ?? record.receivedBytes
                if record.totalBytes == 0 { record.totalBytes = size ?? 0 }
                record.error = nil
            }
        }
    }

    nonisolated func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        didCompleteWithError error: Error?
    ) {
        guard let error else { return }
        let identifier = task.taskIdentifier
        Task { @MainActor in
            guard let uuid = self.tasks[identifier] else { return }
            self.tasks[identifier] = nil
            await self.update(uuid: uuid) { record in
                record.state = .failed
                record.error = error.localizedDescription
            }
        }
    }
}
