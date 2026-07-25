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
            guard rate != oldValue else { return }
            if isPlaying { player?.rate = Float(rate) }
            Task { await persistRate() }
        }
    }

    /// Minutes remaining on the sleep timer, `nil` when off.
    private(set) var sleepMinutesRemaining: Int?

    private var player: AVPlayer?
    private var timeObserver: Any?
    private var endObserver: NSObjectProtocol?
    private var sleepTask: Task<Void, Never>?
    private var sessionStart: Date?
    private var listenedSeconds: Double = 0
    private var lastPersist = Date.distantPast

    private init() {
        configureRemoteCommands()
    }

    var chapters: [ChapterInfo] { manifest?.chapters ?? [] }

    var currentChapter: ChapterInfo? {
        chapters.last { position >= $0.startSeconds }
    }

    var isActive: Bool { book != nil }

    // MARK: - Loading

    func load(book: Book, autoplay: Bool = true) async {
        // Re-opening the book that's already loaded should not restart it.
        if self.book?.uuid == book.uuid, player != nil {
            if autoplay, !isPlaying { play() }
            return
        }

        teardown()
        self.book = book
        isLoading = true
        error = nil
        position = 0
        duration = 0

        do {
            let manifest = try await loadManifest(uuid: book.uuid)
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

            rate = await loadRate(uuid: book.uuid)
            observe(player: player)

            // Resume where the server (or the offline replica) left off.
            if let record = await UserDataService.progress(uuid: book.uuid),
               record.format == .audio, let saved = record.audioPositionSeconds, saved > 1 {
                await seek(to: saved)
            }

            isLoading = false
            if autoplay { play() }
            updateNowPlaying()
        } catch {
            isLoading = false
            self.error = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }

    private func loadManifest(uuid: String) async throws -> AudiobookManifest {
        try await Cache.networkFirst(CacheKey.manifest(uuid)) {
            try await APIClient.shared.get("/api/audiobooks/\(uuid)/manifest")
        }
    }

    private func loadRate(uuid: String) async -> Double {
        guard let record: AudiobookPlaybackRateRecord =
            try? await APIClient.shared.get("/api/audiobooks/\(uuid)/playback-rate")
        else { return 1.0 }
        return min(3.0, max(0.5, record.playbackRate))
    }

    /// Build the player item. A downloaded book plays from disk; a single
    /// remote part streams directly; multiple parts are stitched into one
    /// composition so the timeline and seeking stay continuous.
    private func makeItem(for manifest: AudiobookManifest, uuid: String) async throws -> AVPlayerItem {
        if let local = DownloadManager.shared.localURL(for: uuid) {
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
        Task { await seek(to: chapter.startSeconds) }
    }

    func nextChapter() {
        guard let next = chapters.first(where: { $0.startSeconds > position + 1 }) else { return }
        seekToChapter(next)
    }

    func previousChapter() {
        // Within the first few seconds of a chapter, go to the one before it;
        // otherwise restart the current chapter — the usual player convention.
        guard let current = currentChapter else { return }
        if position - current.startSeconds > 3 {
            seekToChapter(current)
        } else if let index = chapters.firstIndex(where: { $0.ordinal == current.ordinal }), index > 0 {
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

    func close() {
        Task {
            await persistPosition(force: true)
            await finishSession()
            Presentation.shared.noteProgressPersisted()
        }
        teardown()
        book = nil
        manifest = nil
        MPNowPlayingInfoCenter.default().nowPlayingInfo = nil
    }

    private func teardown() {
        if let timeObserver { player?.removeTimeObserver(timeObserver) }
        timeObserver = nil
        if let endObserver { NotificationCenter.default.removeObserver(endObserver) }
        endObserver = nil
        player?.pause()
        player = nil
        isPlaying = false
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

    private func persistPosition(force: Bool) async {
        guard let book, position > 0 else { return }
        // Throttled: the observer fires twice a second, the server needs it
        // every few.
        guard force || Date().timeIntervalSince(lastPersist) > 5 else { return }
        lastPersist = Date()
        await UserDataService.saveProgress(
            ProgressUpdate(
                bookUUID: book.uuid, format: .audio, epubCFI: nil, audioPositionSeconds: position
            )
        )
    }

    private func persistRate() async {
        guard let book else { return }
        let update = AudiobookPlaybackRateUpdate(playbackRate: rate)
        do {
            let _: Empty = try await APIClient.shared.put(
                "/api/audiobooks/\(book.uuid)/playback-rate", body: update
            )
        } catch {
            await SyncEngine.shared.enqueue(
                kind: OpKind.playbackRate(book.uuid),
                path: "/api/audiobooks/\(book.uuid)/playback-rate",
                method: "PUT", body: update, coalesce: true
            )
        }
    }

    private func finishSession() async {
        guard let book, let start = sessionStart, listenedSeconds >= 5 else {
            sessionStart = nil
            listenedSeconds = 0
            return
        }
        let report = SessionReport(
            bookUUID: book.uuid,
            format: .audio,
            startedAt: Int64(start.timeIntervalSince1970),
            endedAt: Int64(Date().timeIntervalSince1970),
            progressUnits: Int64(listenedSeconds),
            deviceId: nil
        )
        sessionStart = nil
        listenedSeconds = 0
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
