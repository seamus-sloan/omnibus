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

@Observable
@MainActor
final class AudioPlayer {
    static let shared = AudioPlayer()

    private(set) var book: Book?
    private(set) var manifest: AudiobookManifest?
    private(set) var isPlaying = false
    private(set) var isLoading = false
    private(set) var duration: Double = 0
    private(set) var error: String?

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

    /// Minutes remaining on the sleep timer, `nil` when off.
    private(set) var sleepMinutesRemaining: Int?

    /// A further position another device reached, waiting on the listener to
    /// accept it. Never applied on its own.
    private(set) var syncOffer: Double?

    private var player: AVPlayer?
    private var timeObserver: Any?
    private var endObserver: NSObjectProtocol?
    private var sleepTask: Task<Void, Never>?
    private var sessionStart: Date?
    private var listenedSeconds: Double = 0
    private var lastPersist = Date.distantPast

    /// Whether the opening position has been settled against the server.
    ///
    /// The periodic observer starts firing as soon as the player has an item,
    /// and every write it makes carries a clock newer than anything already
    /// stored — so a write made before this is true would push a position the
    /// reconcile was still in the middle of correcting, and win.
    private var positionSettled = false

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

    func load(book: Book, autoplay: Bool = true) async {
        // Re-opening the book that's already loaded should not restart it.
        if self.book?.uuid == book.uuid, player != nil {
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

        do {
            let manifest = try await LibraryService.audiobookManifest(uuid: book.uuid)
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
            let local = await UserDataService.localProgress(uuid: book.uuid, format: .audio)
            let remote = Task { @MainActor in
                await PositionSync.newerRemote(uuid: book.uuid, format: .audio, than: local)
            }
            let opening = await firstResult(of: remote, within: PositionSync.openDeadline) ?? local
            if let saved = opening?.audioPositionSeconds, saved > 1 {
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
        if let local = DownloadManager.shared.localURL(for: uuid, kind: .audio) {
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
        await player.seek(
            to: CMTime(seconds: clamped, preferredTimescale: 600),
            toleranceBefore: .zero, toleranceAfter: .zero
        )
        position = clamped
        updateNowPlaying()
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

    func startSleepTimer(minutes: Int) {
        sleepTask?.cancel()
        sleepMinutesRemaining = minutes
        sleepTask = Task { [weak self] in
            for remaining in stride(from: minutes, through: 1, by: -1) {
                try? await Task.sleep(for: .seconds(60))
                if Task.isCancelled { return }
                await MainActor.run { self?.sleepMinutesRemaining = remaining - 1 }
            }
            await MainActor.run {
                self?.pause()
                self?.sleepMinutesRemaining = nil
            }
        }
    }

    func cancelSleepTimer() {
        sleepTask?.cancel()
        sleepTask = nil
        sleepMinutesRemaining = nil
    }

    // MARK: - Lifecycle

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
        let pending = takePendingReport()

        teardown()
        book = nil
        manifest = nil
        position = 0
        duration = 0
        MPNowPlayingInfoCenter.default().nowPlayingInfo = nil

        guard let closing else { return }
        Task {
            if finalPosition > 0 {
                await UserDataService.saveProgress(
                    ProgressUpdate(
                        bookUUID: closing.uuid, format: .audio,
                        epubCFI: nil, audioPositionSeconds: finalPosition
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
    }

    private func observe(player: AVPlayer) {
        timeObserver = player.addPeriodicTimeObserver(
            forInterval: CMTime(seconds: 0.5, preferredTimescale: 600), queue: .main
        ) { [weak self] time in
            Task { @MainActor in
                guard let self else { return }
                self.position = time.seconds
                if self.isPlaying { self.listenedSeconds += 0.5 }
                await self.persistPosition(force: false)
                self.updateNowPlaying()
            }
        }

        endObserver = NotificationCenter.default.addObserver(
            forName: .AVPlayerItemDidPlayToEndTime, object: player.currentItem, queue: .main
        ) { [weak self] _ in
            Task { @MainActor in
                self?.isPlaying = false
                await self?.persistPosition(force: true)
                await self?.checkpointSession()
                // Finishing an audiobook is the strongest completion signal we
                // get, so mark it read rather than waiting for a manual tap.
                if let uuid = self?.book?.uuid {
                    await UserDataService.setReadStatus(uuid: uuid, status: .finished)
                }
            }
        }

        try? AVAudioSession.sharedInstance().setActive(true)
    }

    // MARK: - Persistence

    /// Record where the listener is. `force` marks a moment worth a round trip
    /// of its own — a pause, a backgrounding, a close; the steady ticks queue
    /// and go out with the next push.
    private func persistPosition(force: Bool) async {
        // Nothing may be written before the opening position has been settled
        // against the server: every write carries a fresh clock and would beat
        // the very position the reconcile is fetching.
        guard let book, position > 0, positionSettled else { return }
        // Throttled: the observer fires twice a second, the server needs it
        // every few.
        guard force || Date().timeIntervalSince(lastPersist) > 5 else { return }
        lastPersist = Date()
        await UserDataService.saveProgress(
            ProgressUpdate(
                bookUUID: book.uuid, format: .audio, epubCFI: nil, audioPositionSeconds: position
            ),
            push: force
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
