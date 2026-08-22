//  UploadManager.swift
//  Owns book uploads: the queue, the staged copies on disk, and the transfers.
//
//  This lives outside the view on purpose. Staged bytes and in-flight requests
//  outlive a sheet the reader can dismiss at any moment, and when `AddBooksSheet`
//  owned them every exit path had to remember to clean up — which is precisely
//  the class of bug that kept recurring. Here there is one worker, one
//  cancellable handle, and one registry of live directories, so "who releases
//  this copy" has a single answer.
//
//  Uploads are never queued offline (rule 08, test 1): they are library-wide
//  state, so this holds work only for the life of the process.

import Foundation
import Observation

@Observable
@MainActor
final class UploadManager {
    static let shared = UploadManager()

    /// Progress rows, oldest first. Survives the sheet being dismissed.
    private(set) var uploads: [UploadTask] = []
    /// The draft whose confirm sheet should be on screen, if any.
    private(set) var activeDraft: UploadDraft?
    /// Names the picker allowed through that neither endpoint accepts.
    private(set) var unsupported: [String] = []

    /// Batches waiting to be staged and inspected.
    private var queue: [(batch: UploadBatch, task: UploadTask)] = []
    private var worker: Task<Void, Never>?
    /// Serializes commits against each other while letting the next batch's
    /// inspect overlap one in-flight commit — two full payloads at most.
    private var commitTail: Task<Void, Never>?

    /// Staging directories this process still owns. The sweep consults it so a
    /// second sheet cannot delete the bytes of an upload still running.
    private var liveDirectories: Set<URL> = []

    /// Resolved by `confirm`/`cancelActive`, so the worker can wait on a human.
    private var resolution: CheckedContinuation<UploadConfirmation?, Never>?
    /// Resolved by the view's `onDismiss`, so the next draft is never presented
    /// into a dismissal already in flight — the race that wedged the queue.
    private var dismissal: CheckedContinuation<Void, Never>?
    /// True from the moment Add is tapped until that draft's commit is queued,
    /// so neither Add nor Cancel can act on a draft twice.
    private(set) var isSubmitting = false

    /// Set when a commit lands, cleared when the mirror is resynced, so a
    /// multi-book pick pays one sync instead of one per book.
    private var librarySyncPending = false

    private init() {}

    // MARK: - Intake

    /// Group a pick into batches and queue them. Safe to call while a previous
    /// pick is still draining — the new batches join the same queue.
    func enqueue(_ urls: [URL]) {
        let selection = UploadFlow.selection(for: urls)
        unsupported = selection.unsupported
        guard !selection.batches.isEmpty else { return }

        for batch in selection.batches {
            let task = UploadTask(name: batch.displayName)
            uploads.append(task)
            queue.append((batch, task))
        }
        startWorkerIfIdle()
    }

    private func startWorkerIfIdle() {
        guard worker == nil else { return }
        worker = Task { [weak self] in
            await self?.drain()
            self?.worker = nil
        }
    }

    /// One batch at a time, all the way through confirmation.
    ///
    /// The wait on the reader is what bounds disk: staging every picked book up
    /// front held a full copy of each in `tmp` before the reader had agreed to
    /// any of them, which is how an ordinary multi-book pick could fill a phone.
    private func drain() async {
        while !queue.isEmpty {
            if Task.isCancelled { return }
            let (batch, task) = queue.removeFirst()
            await process(batch, task: task)
        }
        await flushLibrarySync()
    }

    private func process(_ batch: UploadBatch, task: UploadTask) async {
        // Registered before the copy starts: a stage that throws part-way has
        // already written the parts it got through, and nothing else knows
        // where they are.
        let directory = UploadService.makeStagingDirectory()
        liveDirectories.insert(directory)

        let staged: UploadBatch
        let confirmation: UploadConfirmation
        do {
            staged = try await UploadService.stage(batch, into: directory)
            confirmation = try await UploadService.inspect(staged)
        } catch {
            task.state = .failed(UploadManager.message(for: error))
            release(directory)
            return
        }
        if Task.isCancelled {
            task.state = .failed("Cancelled.")
            release(directory)
            return
        }

        task.state = .needsDetails
        let draft = UploadDraft(
            batch: staged, directory: directory, confirmation: confirmation, task: task
        )
        guard let confirmed = await present(draft) else {
            uploads.removeAll { $0.id == task.id }
            release(directory)
            return
        }
        task.state = .uploading
        enqueueCommit(draft, as: confirmed)
    }

    // MARK: - Confirmation

    /// Show `draft`'s sheet and suspend until the reader resolves it. Returns
    /// nil when they cancel.
    private func present(_ draft: UploadDraft) async -> UploadConfirmation? {
        activeDraft = draft
        let confirmed = await withCheckedContinuation { continuation in
            resolution = continuation
        }
        activeDraft = nil
        // Wait for the sheet to finish animating away before the worker can
        // produce another draft: `sheet(item:)` silently drops a re-arm that
        // lands mid-dismissal, and the queue then wedges behind its own guard.
        await withCheckedContinuation { continuation in
            dismissal = continuation
        }
        isSubmitting = false
        return confirmed
    }

    /// Commit the active draft under the reader's confirmed metadata.
    func confirm(_ confirmation: UploadConfirmation) {
        guard !isSubmitting, resolution != nil else { return }
        isSubmitting = true
        resolution?.resume(returning: confirmation)
        resolution = nil
    }

    /// Drop the active draft. Ignored once Add has been tapped, so a Cancel in
    /// the dismissal window cannot delete the bytes a commit is reading.
    func cancelActive() {
        guard !isSubmitting, resolution != nil else { return }
        resolution?.resume(returning: nil)
        resolution = nil
    }

    /// Called by the view once the confirm sheet has finished dismissing.
    func sheetDidDismiss() {
        dismissal?.resume()
        dismissal = nil
    }

    // MARK: - Commit

    private func enqueueCommit(_ draft: UploadDraft, as confirmation: UploadConfirmation) {
        let previous = commitTail
        commitTail = Task { [weak self] in
            await previous?.value
            await self?.runCommit(draft, as: confirmation)
        }
    }

    private func runCommit(_ draft: UploadDraft, as confirmation: UploadConfirmation) async {
        do {
            _ = try await UploadService.commit(draft.batch, as: confirmation)
            draft.task.state = .finished
            librarySyncPending = true
            Haptics.success()
        } catch {
            draft.task.state = .failed(UploadManager.message(for: error))
            // The server files and indexes before it validates, and its request
            // timeout can fire after that point — so a thrown error does not
            // prove the library is unchanged.
            if UploadManager.mayHaveReachedTheServer(error) { librarySyncPending = true }
        }
        release(draft.directory)
        if queue.isEmpty { await flushLibrarySync() }
    }

    /// One library invalidation per pick, not one per book: each carries a full
    /// mirror resync, and `LibraryIndex.sync` drops a forced pass outright while
    /// another is running, so per-book calls silently lost the later books.
    private func flushLibrarySync() async {
        guard librarySyncPending else { return }
        librarySyncPending = false
        await UploadService.invalidateLibrary()
    }

    // MARK: - Staging lifetime

    private func release(_ directory: URL) {
        liveDirectories.remove(directory)
        UploadService.discard(directory)
    }

    /// Delete staged copies from runs that are over, leaving anything this
    /// process still owns. A blind sweep of the root deleted the parts of an
    /// upload that was still reading them.
    func sweepAbandonedStaging() {
        let live = liveDirectories
        Task.detached(priority: .utility) {
            UploadService.sweepStaging(keeping: live)
        }
    }

    // MARK: - Errors

    nonisolated static func message(for error: Error) -> String {
        error.localizedDescription
    }

    /// Whether a failure could have left the book filed anyway. A transport
    /// failure means the request never landed, so invalidating the library —
    /// which *deletes* cached reads — would blank the shelf for a reader who is
    /// now offline and cannot refill it.
    nonisolated static func mayHaveReachedTheServer(_ error: Error) -> Bool {
        guard let error = error as? APIError else { return false }
        switch error {
        case .http: return true
        case .decoding: return true
        case .notConfigured, .offline, .transport, .unauthorized: return false
        }
    }
}
