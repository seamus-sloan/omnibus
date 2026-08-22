//  AudioPlayer.swift
//  AVPlayer-backed audiobook playback.
//
//  Replaces the WebView's hls.js path entirely: AVPlayer speaks HLS natively,
//  and routing through `MPNowPlayingInfoCenter` / `MPRemoteCommandCenter` gets
//  lock-screen, Control Center, AirPlay, and CarPlay transport for free —
//  none of which the hybrid build could offer.

import AVFoundation
import Combine
import MediaPlayer
import Observation
import SwiftUI

/// Sleep-timer state, mirroring the web mobile player's `SleepState` so the
/// two clients' sleep sheets offer the same contract.
enum SleepTimer: Equatable {
    case off
    /// Wall-clock countdown: `remaining` seconds tick down once per second;
    /// `preset` is the option (in seconds) that armed it, for the sheet
    /// highlight.
    case countdown(remaining: Int, preset: Int)
    /// Pause when playback reaches `atSeconds` (the current chapter's end at
    /// arm time).
    case endOfChapter(atSeconds: Double)
}

@Observable
@MainActor
final class AudioPlayer {
    static let shared = AudioPlayer()

    /// The cross-format candidate on offer ("you read further"), shown by
    /// the same banner slot as [`syncOffer`].
    private(set) var crossFormatOffer: CrossFormatCandidate?

    private(set) var book: Book?
    private(set) var manifest: AudiobookManifest?
    /// The `book_files` row playback was built against. Resolved on load —
    /// an explicit pick, else the file the saved position was taken in, else
    /// the server's default — and carried on every position write so a
    /// resume reopens this file rather than the book's first one.
    private(set) var fileID: Int64?
    private(set) var isPlaying = false
    private(set) var isLoading = false
    private(set) var duration: Double = 0
    private(set) var error: String?
    /// Automatic read-status transitions for the loaded book — the same
    /// tracker the readers use, so listening and reading move a book through
    /// the lifecycle identically. Rebuilt on every load; `nil` between books.
    private var autoStatus: ReadStatusAuto?
    /// Whether this load has already claimed the `unread` → `reading`
    /// transition. `play()` runs on every resume from pause; the book is only
    /// started once.
    private var markedReading = false

    /// Current position on the whole-book timeline, in seconds.
    var position: Double = 0

    var rate: Double = 1.0 {
        didSet {
            guard rate != oldValue, !isAdoptingRate else { return }
            if isPlaying { player?.rate = Float(rate) }
            Task { await persistRate() }
        }
    }

    /// Set while a rate that came *out* of storage is being applied, so the
    /// `didSet` above doesn't write it straight back.
    ///
    /// `rate` is a preference the listener sets, and the `didSet` is how that
    /// reaches the server — but `load` assigns to the same property, and `rate`
    /// is singleton state that `teardown` leaves alone, so opening a second
    /// book fired it with the first book's value still in `oldValue`. Online
    /// that was a redundant echo of what had just been read. Offline it was
    /// destructive: with no cached rate for the new book, `loadRate` falls back
    /// to 1.0, and the echo queued a write that reset a speed the listener had
    /// set on another device to 1.0 the moment the device reconnected.
    private var isAdoptingRate = false

    /// Sleep-timer state; `.off` when disarmed.
    private(set) var sleepTimer: SleepTimer = .off

    /// A further position another device reached, waiting on the listener to
    /// accept it. Never applied on its own.
    private(set) var syncOffer: Double?

    private var player: AVPlayer?
    private var timeObserver: Any?
    private var endObserver: NSObjectProtocol?
    private var sleepTask: Task<Void, Never>?
    private var sessionStart: Date?
    private var listenedSeconds: Double = 0
    /// Governs whether a tick is also pushed over the network, on top of the
    /// unconditional local write every one of them gets (#1666). No echoed
    /// restore position to suppress here — ticks begin only once real
    /// playback has resumed — so it opts out of `suppressFirst`.
    private var pushThrottle = PositionPushThrottle(interval: 5, suppressFirst: false)

    /// Whether the opening position has been settled against the server.
    ///
    /// The periodic observer starts firing as soon as the player has an item,
    /// and every write it makes carries a clock newer than anything already
    /// stored — so a write made before this is true would push a position the
    /// reconcile was still in the middle of correcting, and win.
    private var positionSettled = false

    /// Count of `seek(to:)` calls currently awaiting AVPlayer, incremented
    /// before the frame-accurate seek starts and decremented once it
    /// resolves — a count rather than a flag so two overlapping seeks (a
    /// second tap landing before the first settles) can't have the first
    /// one's completion re-arm the observer while the second is still in
    /// flight.
    ///
    /// `seek(to:)` sets `position` optimistically before awaiting AVPlayer,
    /// but the periodic time observer below fires regardless, on the
    /// player's *actual* (pre-seek) time — unguarded, it would overwrite the
    /// optimistic value right back to stale on its very next 0.5s tick,
    /// which for a seek that takes longer than that to settle (HLS
    /// especially) reintroduces the exact staleness the optimism was meant
    /// to close (#1746). The observer checks this count and skips its write
    /// while it's nonzero.
    private var pendingSeekCount = 0

    /// Bumped by every `teardown()` — a book switch or `close()`. A
    /// `seek(to:)` in flight when that happens is abandoned: its `player`
    /// local still resolves once AVPlayer gets around to it, but by then
    /// `pendingSeekCount` belongs to whatever book loaded next. Each seek
    /// captures the generation live when it started and only decrements the
    /// count if that generation is still current, so an orphaned seek's late
    /// completion can't under- or over-count the next book's own in-flight
    /// seeks. `teardown()` separately zeroes the count itself — without that,
    /// a seek abandoned mid-flight would leave it permanently elevated, since
    /// this guard is precisely what stops that seek from ever decrementing it.
    private var loadGeneration = 0

    /// Chapter geometry for the book that's open. Rebuilt once per load — every
    /// lookup on it runs off a half-second time observer, so it can't afford to
    /// re-sort per tick.
    private(set) var timeline = ChapterTimeline()

    private init() {
        configureRemoteCommands()
        syncChapterCommands()
    }

    /// Point the lock screen's chapter-skip buttons at whether there are chapters
    /// to skip. A book with no marks would otherwise offer two controls that
    /// return without doing anything.
    ///
    /// Called from every place `timeline` is assigned — that pairing is the whole
    /// invariant, since the commands are process-wide and outlive any one book.
    private func syncChapterCommands() {
        let center = MPRemoteCommandCenter.shared()
        center.nextTrackCommand.isEnabled = hasChapters
        center.previousTrackCommand.isEnabled = hasChapters
    }

    var isActive: Bool { book != nil }

    // MARK: - Chapters

    var chapters: [ChapterInfo] { timeline.chapters }

    var hasChapters: Bool { !timeline.isEmpty }

    var currentChapterIndex: Int? { timeline.index(at: position) }

    var currentChapter: ChapterInfo? { timeline.chapter(at: position) }

    /// Start of the span the scrubber covers: the current chapter, or the whole
    /// book when there are no chapters to scope it to.
    var chapterStart: Double { timeline.span(at: position).start }

    /// Length of that span.
    var chapterDuration: Double { timeline.span(at: position).duration }

    /// How far into the span playback has got.
    var chapterOffset: Double { max(0, position - chapterStart) }

    func chapterLength(at index: Int) -> Double { timeline.length(at: index) }

    var canGoNextChapter: Bool {
        guard let index = currentChapterIndex else { return false }
        return index + 1 < timeline.count
    }

    /// Previous is available in the first chapter too — there it restarts,
    /// which is the verb the button has in every other player.
    var canGoPreviousChapter: Bool { hasChapters }

    // MARK: - Loading

    func load(book: Book, fileID requestedFileID: Int64? = nil, autoplay: Bool = true) async {
        // Re-opening the book that's already loaded should not restart it —
        // unless a different file of it was explicitly asked for. A `nil`
        // request means "whatever is right", and what's already playing is.
        if self.book?.uuid == book.uuid, player != nil,
           requestedFileID == nil || requestedFileID == fileID
        {
            if autoplay, !isPlaying { play() }
            return
        }

        // Captured before `teardown`, which clears the book this session
        // belongs to. Sent without waiting — switching books shouldn't stall
        // behind a report for the one being left.
        if let pending = takePendingReport() {
            Task { await UserDataService.reportSessions([pending]) }
        }
        teardown()
        self.book = book
        autoStatus = ReadStatusAuto(uuid: book.uuid)
        markedReading = false
        // Backgrounding mid-chapter has to persist the position the same way
        // pausing does — the player keeps running behind the lock screen, so
        // "the app went away" is not the same event as "playback stopped".
        //
        // The session is checkpointed here too. Playback survives backgrounding,
        // so this is not the end of listening — but it is the last moment the
        // app is guaranteed to run before the system may kill it, and a report
        // covering only what has been listened to so far is exactly what makes
        // the eventual total right either way.
        LifecycleSync.shared.register(self) { [weak self] in
            await self?.persistPosition(force: true)
            await self?.checkpointSession()
        }
        LifecycleSync.shared.didOpenBook()
        isLoading = true
        error = nil
        position = 0
        duration = 0

        // The saved position is read before the manifest because it also
        // names the file to open: an explicit pick wins, else the file the
        // position was taken in, else the server's default (first by
        // ordinal). Resolved up front — the manifest is built per file.
        let local = await UserDataService.localProgress(uuid: book.uuid, format: .audio)
        fileID = requestedFileID ?? local?.bookFileID ?? book.audioFiles.first?.id

        do {
            let manifest = try await loadManifest(for: book)
            self.manifest = manifest
            let item = try await makeItem(for: manifest, uuid: book.uuid)

            let player = AVPlayer(playerItem: item)
            player.automaticallyWaitsToMinimizeStalling = true
            self.player = player

            if let total = manifest.totalDuration {
                duration = total
            } else if let assetDuration = try? await item.asset.load(.duration) {
                duration = assetDuration.seconds
            } else {
                duration = 0
            }
            if !duration.isFinite { duration = 0 }

            // Built after `duration`, not with the manifest: a chapter that ships
            // no length of its own measures to the next chapter's start, and the
            // last one has only the end of the book to measure to.
            timeline = ChapterTimeline(chapters: manifest.chapters, bookDuration: duration)
            syncChapterCommands()

            adoptRate(await loadRate(uuid: book.uuid))

            // Where this book is up to, settled before a single frame plays.
            //
            // This used to read the replica and nothing else, which made the
            // player the one surface with no idea another device existed — and
            // not merely behind, but destructive: the periodic observer below
            // starts writing within half a second, stamped with a clock newer
            // than anything already on the server, so a position reached
            // elsewhere was overwritten by a stale local one before the
            // listener had heard a word. Reading first is what makes the
            // handoff work in the direction it was always claimed to.
            let remote = Task { @MainActor in
                await PositionSync.newerRemote(uuid: book.uuid, format: .audio, than: local)
            }
            let opening = await firstResult(of: remote, within: PositionSync.openDeadline) ?? local
            if let saved = opening?.audioPositionSeconds, saved > 1, matchesLoadedFile(opening) {
                await seek(to: saved)
            }
            // Only now may the observer write. Before this the position is
            // still 0 or a stale resume, and either would win on the wire.
            positionSettled = true

            observe(player: player)
            isLoading = false
            if autoplay { play() }
            updateNowPlaying()

            // Anything that lands after the deadline is not applied — yanking
            // the timeline out from under someone already listening is worse
            // than being behind — but it is offered, the same way the reader
            // offers it.
            offerLatePosition(from: remote, uuid: book.uuid)
            offerCrossFormat()
        } catch {
            isLoading = false
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// Apply a rate that came out of storage without writing it back. See
    /// [`isAdoptingRate`].
    private func adoptRate(_ value: Double) {
        isAdoptingRate = true
        rate = value
        isAdoptingRate = false
    }

    /// Apply a rate audibly without persisting it — the speed slider's live
    /// drag path, where per-tick writes would spam the server. Pair with
    /// [`commitRate`] on drag end.
    func setRateLive(_ value: Double) {
        isAdoptingRate = true
        rate = value
        isAdoptingRate = false
        if isPlaying { player?.rate = Float(value) }
    }

    /// Persist the current rate — the drag-end commit for [`setRateLive`].
    func commitRate() {
        Task { await persistRate() }
    }

    /// The manifest for the resolved file, falling back to the server's
    /// default when a remembered id has gone stale — `book_files` rows churn
    /// on reindex, so a position saved before one can name a file the book
    /// no longer has.
    private func loadManifest(for book: Book) async throws -> AudiobookManifest {
        do {
            return try await LibraryService.audiobookManifest(uuid: book.uuid, fileID: fileID)
        } catch let error as APIError {
            guard case .http(status: 404, message: _) = error, fileID != nil else { throw error }
            fileID = book.audioFiles.first?.id
            return try await LibraryService.audiobookManifest(uuid: book.uuid)
        }
    }

    /// Whether a stored position belongs to the file being played. A record
    /// carrying no file id predates selection (or came from an older server)
    /// and applies to whatever is loaded — the pre-selection behaviour.
    private func matchesLoadedFile(_ record: ProgressRecord?) -> Bool {
        guard let recorded = record?.bookFileID, let fileID else { return true }
        return recorded == fileID
    }

    /// Whether the resolved file is the one the server picks with no
    /// `file_id` at all — the first audio file by ordinal. Answered `true`
    /// when the cached `Book` doesn't carry its files: refusing the local
    /// copy on missing data would break offline playback of a downloaded
    /// book, which is worse than opening a mis-remembered selection.
    private var isDefaultFileSelected: Bool {
        guard let fileID, let defaultID = book?.audioFiles.first?.id else { return true }
        return fileID == defaultID
    }

    /// Surface a position that landed after the open deadline, if it is still
    /// ahead of where this listener has got to.
    ///
    /// Applied silently it would be a seek out of nowhere, seconds into a
    /// chapter someone is already following. Left unmentioned it would be lost
    /// the moment this device's own position write goes out. So it is offered,
    /// and the offer is withdrawn on its own if the listener plays past it.
    private func offerLatePosition(from remote: Task<ProgressRecord?, Never>, uuid: String) {
        Task { @MainActor in
            guard let record = await remote.value,
                  let seconds = record.audioPositionSeconds,
                  book?.uuid == uuid,
                  matchesLoadedFile(record),
                  seconds > position + 1
            else { return }
            withAnimation(Motion.settle) { syncOffer = seconds }
        }
    }

    /// Take the offered position. Seeking is enough — the next periodic write
    /// carries it, and it now has this device's clock behind it.
    func acceptSyncOffer() async {
        guard let seconds = syncOffer else { return }
        dismissSyncOffer()
        await seek(to: seconds)
        await persistPosition(force: true)
    }

    /// Declining is as final as accepting: re-offering on the next tick would
    /// nag through the whole chapter.
    func dismissSyncOffer() {
        withAnimation(Motion.settle) { syncOffer = nil }
    }

    /// Fetch the cross-format candidate for the loaded book (best-effort
    /// network read; rule 08 — nothing here ever queues). Offered with the
    /// same banner mechanics as the same-format late-position offer, and
    /// only when the mapped file is the one playing — the candidate's
    /// seconds are within that file alone.
    func offerCrossFormat() {
        guard let uuid = book?.uuid else { return }
        Task { @MainActor in
            guard let resume = try? await UserDataService.crossFormatResume(uuid: uuid, target: .audio),
                  resume.state == .candidate,
                  let candidate = resume.candidate,
                  book?.uuid == uuid,
                  syncOffer == nil,
                  SyncPromptStore.fileMatches(
                      selected: fileID,
                      defaultID: book?.audioFiles.first?.id,
                      candidate: candidate.bookFileID
                  )
            else { return }
            if resume.follow == true, let seconds = candidate.audioPositionSeconds {
                // Follow mode: resolve-at-open — apply the mapped position
                // silently, no banner, no dismissal bookkeeping.
                await seek(to: seconds)
                return
            }
            let seen = await SyncPromptStore.dismissedClock(uuid: uuid, target: .audio)
            guard SyncPromptStore.shouldOffer(
                sourceClock: candidate.sourceClientUpdatedAt,
                dismissed: seen
            ) else { return }
            withAnimation(Motion.settle) { crossFormatOffer = candidate }
        }
    }

    /// "Synced here": declare the current listening position as a sync
    /// point (rule 08 — direct call, never queued; the control is disabled
    /// offline). Returns whether the declaration was accepted.
    func declareSyncPoint() async -> Bool {
        guard let uuid = book?.uuid else { return false }
        let decl = DeclareSyncPoint(
            bookUUID: uuid,
            format: .audio,
            ebookFraction: nil,
            audioBookFileID: fileID ?? book?.audioFiles.first?.id,
            audioSeconds: position
        )
        do {
            try await UserDataService.declareSyncPoint(decl)
            return true
        } catch {
            return false
        }
    }

    /// Take the cross-format offer: seek, then force a write so the jump
    /// carries this device's clock (the normal position path).
    func acceptCrossFormatOffer() async {
        guard let candidate = crossFormatOffer,
              let seconds = candidate.audioPositionSeconds
        else { return }
        await dismissCrossFormatOffer()
        await seek(to: seconds)
        await persistPosition(force: true)
    }

    /// Declining stores the source clock so the prompt re-arms only after
    /// the reading position advances. Never a progress write.
    func dismissCrossFormatOffer() async {
        if let candidate = crossFormatOffer, let uuid = book?.uuid {
            await SyncPromptStore.storeDismissedClock(
                candidate.sourceClientUpdatedAt,
                uuid: uuid,
                target: .audio
            )
        }
        withAnimation(Motion.settle) { crossFormatOffer = nil }
    }

    /// The saved playback rate, from the replica when the server can't be
    /// reached.
    ///
    /// This was a bare network read with a `1.0` fallback, so opening a
    /// downloaded audiobook offline silently threw away the speed the listener
    /// had set — while the *write* path queued correctly. Settled rather than
    /// live for the same reason as the manifest: the rate is applied to a
    /// player being built, not to a view that can update underneath itself.
    private func loadRate(uuid: String) async -> Double {
        let record: AudiobookPlaybackRateRecord? = try? await Cache.settled(
            CacheKey.playbackRate(uuid)
        ) {
            try await APIClient.shared.get("/api/audiobooks/\(uuid)/playback-rate")
        }
        guard let record else { return 1.0 }
        return min(3.0, max(0.5, record.playbackRate))
    }

    /// Build the player item. A downloaded book plays from disk; a single
    /// remote part streams directly; multiple parts are stitched into one
    /// composition so the timeline and seeking stay continuous.
    private func makeItem(for manifest: AudiobookManifest, uuid: String) async throws -> AVPlayerItem {
        // The downloaded copy is the server's default file (the download
        // endpoint takes no file id), so it can only stand in for that
        // selection — any other file of the book streams.
        if isDefaultFileSelected, let local = DownloadManager.shared.localURL(for: uuid, kind: .audio) {
            return AVPlayerItem(asset: AVURLAsset(url: local))
        }

        let headers = await APIClient.shared.authHeaders()
        let options: [String: Any] = headers.isEmpty ? [:] : ["AVURLAssetHTTPHeaderFieldsKey": headers]

        switch manifest {
        case let .hls(playlistURL):
            guard let url = await APIClient.shared.absoluteURL(playlistURL) else {
                throw APIError.notConfigured
            }
            return AVPlayerItem(asset: AVURLAsset(url: url, options: options))

        case let .direct(parts, _, _):
            guard !parts.isEmpty else { throw APIError.http(status: 404, message: "No audio parts.") }
            let sorted = parts.sorted { $0.ordinal < $1.ordinal }

            if sorted.count == 1 {
                guard let url = await APIClient.shared.absoluteURL(sorted[0].url) else {
                    throw APIError.notConfigured
                }
                return AVPlayerItem(asset: AVURLAsset(url: url, options: options))
            }

            let composition = AVMutableComposition()
            guard let track = composition.addMutableTrack(
                withMediaType: .audio, preferredTrackID: kCMPersistentTrackID_Invalid
            ) else { throw APIError.http(status: 500, message: "Could not build the audio timeline.") }

            var cursor = CMTime.zero
            for part in sorted {
                guard let url = await APIClient.shared.absoluteURL(part.url) else { continue }
                let asset = AVURLAsset(url: url, options: options)
                guard let source = try await asset.loadTracks(withMediaType: .audio).first else { continue }
                let assetDuration = try await asset.load(.duration)
                try track.insertTimeRange(
                    CMTimeRange(start: .zero, duration: assetDuration), of: source, at: cursor
                )
                cursor = CMTimeAdd(cursor, assetDuration)
            }
            return AVPlayerItem(asset: composition)
        }
    }

    // MARK: - Transport

    func play() {
        guard let player else { return }
        player.rate = Float(rate)
        isPlaying = true
        if sessionStart == nil { sessionStart = Date() }
        updateNowPlaying()
        // Listening to a book is starting it, the audio counterpart of the
        // readers' mark-on-open. Off the critical path — the fetch behind it
        // may cost a round trip and playback has already begun.
        if !markedReading {
            markedReading = true
            // Resolved now, not inside the task. Reading `self.autoStatus`
            // when the task eventually runs would pick up whatever `load`
            // has installed by then, marking a book the listener switched to
            // — and never played — as started.
            let tracker = autoStatus
            Task { await tracker?.bookOpened() }
        }
    }

    func pause() {
        player?.pause()
        isPlaying = false
        updateNowPlaying()
        Task {
            await persistPosition(force: true)
            // Stopping is the one moment listening time is definitely complete.
            // Reporting only from `close()` meant reporting never — nothing
            // calls it — so every listening session this app has ever recorded
            // was lost and the stats screen counted audiobooks as zero.
            await checkpointSession()
            Presentation.shared.noteProgressPersisted()
        }
    }

    func toggle() {
        Haptics.tap()
        isPlaying ? pause() : play()
    }

    func seek(to seconds: Double) async {
        guard let player else { return }
        let clamped = max(0, duration > 0 ? min(seconds, duration) : seconds)
        // Applied before the frame-accurate AVPlayer seek resolves, not after.
        // `toleranceBefore/After: .zero` can take real wall-clock time to
        // settle (HLS especially), and every reader of `position` — the
        // chapter-scoped scrubber's own `chapterOffset` included, everywhere
        // it isn't shadowed by the drag-local `scrubOffset` — was reading the
        // pre-seek value for that whole window. The mini bar has no such
        // shadow, so a skip or chapter jump left its whole-book rail visibly
        // behind the position the rest of the player already claimed to be
        // at (#1746).
        position = clamped
        updateNowPlaying()
        // Guards the observer for the rest of this function, however it
        // exits. The generation is captured now, not read fresh in the
        // `defer` — see `loadGeneration`.
        let generation = loadGeneration
        pendingSeekCount += 1
        defer {
            if generation == loadGeneration { pendingSeekCount -= 1 }
        }
        await player.seek(
            to: CMTime(seconds: clamped, preferredTimescale: 600),
            toleranceBefore: .zero, toleranceAfter: .zero
        )
    }

    func skip(_ delta: Double) {
        Haptics.tap()
        Task { await seek(to: position + delta) }
    }

    func seekToChapter(_ chapter: ChapterInfo) {
        Haptics.tap()
        Task { await seek(to: chapter.startSeconds) }
    }

    /// Seek to an offset inside the current chapter — what the chapter-scoped
    /// scrubber hands back.
    func seekWithinChapter(to offset: Double) async {
        await seek(to: chapterStart + offset)
    }

    func nextChapter() {
        guard let index = currentChapterIndex, index + 1 < chapters.count else { return }
        seekToChapter(chapters[index + 1])
    }

    func previousChapter() {
        // Within the first few seconds of a chapter, go to the one before it;
        // otherwise restart the current chapter — the usual player convention.
        guard let index = currentChapterIndex else { return }
        let current = chapters[index]
        if position - current.startSeconds > 3 || index == 0 {
            seekToChapter(current)
        } else {
            seekToChapter(chapters[index - 1])
        }
    }

    // MARK: - Sleep timer

    /// Arm a wall-clock countdown; `seconds <= 0` disarms.
    func startSleepTimer(seconds: Int) {
        sleepTask?.cancel()
        sleepTask = nil
        guard seconds > 0 else {
            sleepTimer = .off
            return
        }
        sleepTimer = .countdown(remaining: seconds, preset: seconds)
        sleepTask = Task { [weak self] in
            for remaining in stride(from: seconds - 1, through: 0, by: -1) {
                try? await Task.sleep(for: .seconds(1))
                if Task.isCancelled { return }
                await MainActor.run {
                    self?.sleepTimer = .countdown(remaining: remaining, preset: seconds)
                }
            }
            await MainActor.run {
                self?.pause()
                self?.sleepTimer = .off
            }
        }
    }

    /// Arm the timer to pause at `atSeconds` — the current chapter's end,
    /// frozen at arm time. The periodic observer enforces the boundary.
    func startSleepTimer(endOfChapterAt atSeconds: Double) {
        sleepTask?.cancel()
        sleepTask = nil
        sleepTimer = .endOfChapter(atSeconds: atSeconds)
    }

    func cancelSleepTimer() {
        sleepTask?.cancel()
        sleepTask = nil
        sleepTimer = .off
    }

    /// Seconds left on the sleep timer for display, deriving the
    /// end-of-chapter variant from the playback position. `nil` when off.
    /// Mirrors the web sheet's `sleep_remaining`.
    nonisolated static func sleepRemaining(_ timer: SleepTimer, position: Double) -> Int? {
        switch timer {
        case .off:
            return nil
        case .countdown(let remaining, _):
            return max(0, remaining)
        case .endOfChapter(let atSeconds):
            return Int(max(0, atSeconds - position).rounded(.up))
        }
    }

    var sleepRemainingSeconds: Int? { Self.sleepRemaining(sleepTimer, position: position) }

    // MARK: - Lifecycle

    /// Whether a close-time flush is safe to write, mirroring the guard
    /// `persistPosition` applies on every tick: nothing may leave this device
    /// before the opening position has been reconciled against the server,
    /// because every write carries a clock newer than anything already
    /// stored and would beat a genuinely further-along position still in
    /// flight from another device.
    nonisolated static func shouldPersistOnClose(settled: Bool, finalPosition: Double) -> Bool {
        settled && finalPosition > 0
    }

    /// Stop playback and put the player away — what the mini bar's dismiss
    /// means. The position and the session are flushed before the book is
    /// released, since both are keyed on it.
    func close() {
        // Everything the flush needs, captured before the teardown clears it.
        // Doing the flush inside a `Task` and tearing down around it doesn't
        // work in either order: the `Task` body only runs once this synchronous
        // frame ends, so putting the teardown first leaves the flush with no
        // book to attribute, and putting it last leaves the mini bar on screen
        // until the network answers.
        let closing = book
        let finalPosition = position
        let finalFileID = fileID
        // Captured before `teardown`, which resets it — matching the same
        // guard `persistPosition` applies on every tick. Without it, closing
        // during the open-time reconcile window stamps whatever the player
        // happened to be sitting at (0, or a stale resume) with a fresh
        // clock, which can beat — and coalesce-delete — a genuinely newer
        // position still in flight from another device.
        let settled = positionSettled
        let pending = takePendingReport()

        teardown()
        book = nil
        manifest = nil
        fileID = nil
        position = 0
        duration = 0
        MPNowPlayingInfoCenter.default().nowPlayingInfo = nil

        guard let closing else { return }
        Task {
            if Self.shouldPersistOnClose(settled: settled, finalPosition: finalPosition) {
                await UserDataService.saveProgress(
                    ProgressUpdate(
                        bookUUID: closing.uuid, format: .audio,
                        epubCFI: nil, audioPositionSeconds: finalPosition,
                        bookFileID: finalFileID
                    )
                )
            }
            if let pending { await UserDataService.reportSessions([pending]) }
            Presentation.shared.noteProgressPersisted()
            LifecycleSync.shared.didCloseBook()
        }
    }

    private func teardown() {
        LifecycleSync.shared.unregister(self)
        if let timeObserver { player?.removeTimeObserver(timeObserver) }
        timeObserver = nil
        if let endObserver { NotificationCenter.default.removeObserver(endObserver) }
        endObserver = nil
        player?.pause()
        player = nil
        isPlaying = false
        // Whatever these held belonged to the book being torn down; callers
        // checkpoint first, and anything left is not the next book's.
        sessionStart = nil
        listenedSeconds = 0
        // Tracks one book's lifecycle, so it must not outlive it — a stale
        // tracker would apply the previous book's status to the next one.
        autoStatus = nil
        markedReading = false
        // Cleared with the player, not with the book: `load` awaits the next
        // manifest, and leaving this up means the chapter bar spends that window
        // offering to seek inside the book that was just closed.
        timeline = ChapterTimeline()
        // Paired with the line above so the lock screen's chapter buttons can
        // never outlive the chapter data they act on. `load` tears down and then
        // *awaits* the next manifest, so leaving the previous book's enablement
        // standing offered chapter skip across that whole window — and with an
        // empty timeline both commands return without doing anything.
        syncChapterCommands()
        // Closed again so the next book cannot be written to before its own
        // opening position has been settled — this is the one flag whose stale
        // value would be silently destructive rather than merely wrong.
        positionSettled = false
        syncOffer = nil
        cancelSleepTimer()
        // Bumped first so a seek belonging to the book being torn down —
        // still holding its own `defer`, due to land whenever AVPlayer gets
        // around to it — finds a mismatched generation and skips its
        // decrement instead of under-counting whatever book loads next. The
        // explicit reset is still required alongside it: that guard is
        // exactly what stops the orphaned seek from ever decrementing this,
        // so without the reset a seek abandoned mid-flight would leave the
        // count permanently elevated and the observer permanently skipped.
        loadGeneration += 1
        pendingSeekCount = 0
    }

    private func observe(player: AVPlayer) {
        timeObserver = player.addPeriodicTimeObserver(
            forInterval: CMTime(seconds: 0.5, preferredTimescale: 600), queue: .main
        ) { [weak self] time in
            Task { @MainActor in
                guard let self else { return }
                // Skipped while a `seek(to:)` is still resolving: `time` is
                // AVPlayer's actual (pre-seek) position until then, and
                // writing it here would clobber the optimistic target right
                // back to stale for as long as the seek takes to settle.
                guard self.pendingSeekCount == 0 else { return }
                self.position = time.seconds
                // An armed end-of-chapter sleep timer fires the moment
                // playback crosses the boundary it was armed against.
                if case .endOfChapter(let atSeconds) = self.sleepTimer,
                    self.position >= atSeconds
                {
                    self.pause()
                    self.sleepTimer = .off
                }
                // The periodic observer ticks every 0.5 *media* seconds — at
                // 2x that's twice per wall second — so each tick is converted
                // to wall-clock before it accrues. Listening stats count the
                // listener's real time, not the book time covered.
                if self.isPlaying { self.listenedSeconds += Format.atRate(0.5, rate: self.rate) }
                await self.persistPosition(force: false)
                self.updateNowPlaying()
            }
        }

        // Bound to this load, like the observer itself: reading
        // `self.autoStatus` from inside the task would resolve it after a
        // teardown or a book switch had already replaced it, and mark a book
        // nobody listened to `finished` — which now also drops it off the
        // Continue rail.
        let tracker = autoStatus
        endObserver = NotificationCenter.default.addObserver(
            forName: .AVPlayerItemDidPlayToEndTime, object: player.currentItem, queue: .main
        ) { [weak self] _ in
            Task { @MainActor in
                self?.isPlaying = false
                await self?.persistPosition(force: true)
                await self?.checkpointSession()
                // The manifest streams every part of the book as one item, so
                // this fires once the last file has played out — finishing an
                // audiobook is the strongest completion signal we get, and it
                // goes through the same tracker as the readers rather than
                // writing `finished` blind.
                await tracker?.positionChanged(atEnd: true)
            }
        }

        try? AVAudioSession.sharedInstance().setActive(true)
    }

    // MARK: - Persistence

    /// Record where the listener is. The write into the replica and the
    /// outbox happens on every tick, unconditionally — cheap even at twice a
    /// second, since `SyncEngine.record` coalesces on kind (#1666). `force`
    /// decides only whether this tick is also worth a round trip of its own —
    /// a pause, a backgrounding, a close — as opposed to the steady ticks,
    /// which still queue immediately but push at most once every five
    /// seconds.
    private func persistPosition(force: Bool) async {
        // Nothing may be written before the opening position has been settled
        // against the server: every write carries a fresh clock and would beat
        // the very position the reconcile is fetching.
        guard let book, position > 0, positionSettled else { return }
        let push = pushThrottle.shouldPush(force: force)
        await UserDataService.saveProgress(
            ProgressUpdate(
                bookUUID: book.uuid, format: .audio, epubCFI: nil,
                audioPositionSeconds: position, bookFileID: fileID
            ),
            push: push
        )
    }

    private func persistRate() async {
        guard let book else { return }
        let update = AudiobookPlaybackRateUpdate(playbackRate: rate)
        // Write through to the replica first, so the rate survives a relaunch
        // made offline rather than reverting to 1.0 on the next open.
        await Cache.write(
            CacheKey.playbackRate(book.uuid),
            AudiobookPlaybackRateRecord(
                bookUUID: book.uuid,
                playbackRate: rate,
                updatedAt: Int64(Date().timeIntervalSince1970)
            )
        )
        await SyncEngine.shared.write(
            kind: OpKind.playbackRate(book.uuid),
            path: "/api/audiobooks/\(book.uuid)/playback-rate",
            method: "PUT", body: update, coalesce: true
        )
    }

    /// Take the listening accumulated so far as a report, closing the window.
    ///
    /// Taking is split from sending so a caller about to tear the player down
    /// can capture the values first — everything here is cleared by `teardown`,
    /// and reading it afterwards finds nothing to attribute.
    ///
    /// Under five seconds is left to accumulate rather than discarded: a pause
    /// four seconds in should carry into the next stretch, not vanish.
    private func takePendingReport() -> SessionReport? {
        guard let book, sessionStart != nil, listenedSeconds >= 5 else { return nil }
        let ended = Int64(Date().timeIntervalSince1970)
        let units = Int64(listenedSeconds)
        // The span is derived from the listening rather than from when the
        // window opened. `listenedSeconds` counts only the time the player was
        // actually running, and a window that returns nothing (under five
        // seconds) is deliberately left open to keep accumulating — so the
        // opening timestamp drifts arbitrarily far back behind a few paused
        // hours, and a report claiming to start before the app launched buckets
        // into the wrong day. "This much listening, ending now" is the honest
        // shape, and it matches what the reader already reports.
        let report = SessionReport(
            bookUUID: book.uuid,
            format: .audio,
            startedAt: ended - units,
            endedAt: ended,
            progressUnits: units,
            deviceId: nil
        )
        // Cleared here, before any await a caller adds: a second checkpoint
        // racing this one — a pause landing while the backgrounding flush is in
        // flight — would otherwise report the same seconds twice under two
        // client ids, the one shape the server's idempotency can't collapse.
        sessionStart = isPlaying ? Date() : nil
        listenedSeconds = 0
        return report
    }

    /// Report the listening accumulated so far and start a fresh window.
    ///
    /// A checkpoint rather than an end, because there is no single moment that
    /// reliably *is* the end: playback outlives the foreground, and the system
    /// can kill a backgrounded app without warning. Reporting at every point
    /// where listening definitely pauses — a pause, a backgrounding, a book
    /// switch, the end of the book — means the worst a lost app costs is the
    /// seconds since the last checkpoint. Each report covers a disjoint slice
    /// of `listenedSeconds` and carries its own client id, so replaying one
    /// can't double-count either.
    private func checkpointSession() async {
        guard let report = takePendingReport() else { return }
        await UserDataService.reportSessions([report])
    }

    // MARK: - Now Playing / remote commands

    private func updateNowPlaying() {
        guard let book else { return }
        var info: [String: Any] = [
            MPMediaItemPropertyTitle: currentChapter?.title ?? book.displayTitle,
            MPMediaItemPropertyAlbumTitle: book.displayTitle,
            MPMediaItemPropertyArtist: book.authorDisplay,
            MPMediaItemPropertyPlaybackDuration: duration,
            MPNowPlayingInfoPropertyElapsedPlaybackTime: position,
            MPNowPlayingInfoPropertyPlaybackRate: isPlaying ? rate : 0,
            MPNowPlayingInfoPropertyMediaType: MPNowPlayingInfoMediaType.audio.rawValue,
        ]
        if let index = currentChapterIndex {
            info[MPNowPlayingInfoPropertyChapterNumber] = index + 1
            info[MPNowPlayingInfoPropertyChapterCount] = chapters.count
        }
        if let artwork = currentArtwork {
            info[MPMediaItemPropertyArtwork] = artwork
        }
        MPNowPlayingInfoCenter.default().nowPlayingInfo = info
    }

    private var artworkCache: (uuid: String, artwork: MPMediaItemArtwork)?

    private var currentArtwork: MPMediaItemArtwork? {
        guard let book else { return nil }
        if let cached = artworkCache, cached.uuid == book.uuid { return cached.artwork }
        Task { await loadArtwork(for: book) }
        return nil
    }

    private func loadArtwork(for book: Book) async {
        guard book.coverURL != nil, artworkCache?.uuid != book.uuid else { return }
        var resolved = await ImageCache.shared.image(for: "/api/thumbs/\(book.uuid)/lg")
        if resolved == nil,
           let data = try? await APIClient.shared.data(for: "/api/covers/\(book.uuid)") {
            resolved = UIImage(data: data)
        }
        guard let image = resolved else { return }
        let artwork = MPMediaItemArtwork(boundsSize: image.size) { _ in image }
        artworkCache = (book.uuid, artwork)
        updateNowPlaying()
    }

    private func configureRemoteCommands() {
        let center = MPRemoteCommandCenter.shared()

        center.playCommand.addTarget { [weak self] _ in
            Task { @MainActor in self?.play() }
            return .success
        }
        center.pauseCommand.addTarget { [weak self] _ in
            Task { @MainActor in self?.pause() }
            return .success
        }
        center.togglePlayPauseCommand.addTarget { [weak self] _ in
            Task { @MainActor in self?.isPlaying == true ? self?.pause() : self?.play() }
            return .success
        }
        center.skipForwardCommand.preferredIntervals = [30]
        center.skipForwardCommand.addTarget { [weak self] _ in
            Task { @MainActor in self?.skip(30) }
            return .success
        }
        center.skipBackwardCommand.preferredIntervals = [15]
        center.skipBackwardCommand.addTarget { [weak self] _ in
            Task { @MainActor in self?.skip(-15) }
            return .success
        }
        center.nextTrackCommand.addTarget { [weak self] _ in
            Task { @MainActor in self?.nextChapter() }
            return .success
        }
        center.previousTrackCommand.addTarget { [weak self] _ in
            Task { @MainActor in self?.previousChapter() }
            return .success
        }
        center.changePlaybackPositionCommand.addTarget { [weak self] event in
            guard let event = event as? MPChangePlaybackPositionCommandEvent else { return .commandFailed }
            Task { @MainActor in await self?.seek(to: event.positionTime) }
            return .success
        }
    }
}
