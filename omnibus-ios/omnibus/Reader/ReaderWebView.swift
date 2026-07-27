//  ReaderWebView.swift
//  WKWebView host for the vendored epub.js reader.
//
//  The rendering engine is reused wholesale from the web build
//  (`frontend/assets/vendor/epub-reader-glue.js`) — iOS ships no native EPUB
//  renderer, and epub.js already implements CFI positions, pagination, and
//  annotations against the exact contract the server expects. Everything
//  around it (chrome, gestures, sheets, persistence) is native.
//
//  Assets and the book itself are served over a custom `omnibus-reader://`
//  scheme so epub.js sees one same-origin space and needs no cookie.

import SwiftUI
import WebKit

/// Cross-view channel for "open the reader at this position", used by the
/// bookmark sheet, which is presented outside the reader's own hierarchy.
@Observable
@MainActor
final class ReaderBridge {
    static let shared = ReaderBridge()
    var pendingCFI: String?
}

/// Column width. `OmnibusReader.setMargins` assigns this straight to CSS
/// `max-width`, so the values have to be valid CSS lengths — the web build
/// uses these same three percentages.
enum ReaderMargins: String, Codable, CaseIterable {
    case narrow, normal, wide

    var css: String {
        switch self {
        case .narrow: "95%"
        case .normal: "80%"
        case .wide: "65%"
        }
    }

    var label: String {
        switch self {
        case .narrow: "Narrow"
        case .normal: "Normal"
        case .wide: "Wide"
        }
    }
}

struct ReaderSettings: Codable, Equatable {
    var fontSize: Int = 19
    var fontFamily: String = "serif"
    var lineHeight: Double = 1.6
    var margins: ReaderMargins = .normal
    var justify: Bool = true
    /// epub.js theme token — matches the four `themes.register` names.
    var theme: String = "dark"

    static let storageKey = "omnibus.readerSettings"

    static func load() -> ReaderSettings {
        guard let data = UserDefaults.standard.data(forKey: storageKey),
              let decoded = try? JSONDecoder().decode(ReaderSettings.self, from: data)
        else { return ReaderSettings() }
        return decoded
    }

    func save() {
        guard let data = try? JSONEncoder().encode(self) else { return }
        UserDefaults.standard.set(data, forKey: Self.storageKey)
    }
}

struct TocItem: Codable, Hashable, Identifiable {
    var label: String
    var href: String
    var level: Int
    /// Where the entry starts. Absent until epub.js's whole-book locations pass
    /// lands, so the contents list is usable before positions are known.
    var page: Int?
    var pct: Int?
    var id: String { href + label }
}

/// Mirrors `buildRelocateData` in `epub-reader-glue.js`. Note `pct` is an
/// integer 0–100 (not a fraction) and `chapter` is a 1-based index — the
/// human-readable name is `chapterTitle`.
struct RelocateData: Codable {
    var cfi: String?
    var page: Int = 0
    var totalPages: Int = 0
    var pct: Int = 0
    var chapter: Int = 0
    var totalChapters: Int = 0
    var chapterTitle: String = ""
    /// Pages between here and the end of the current chapter; 0 when unknown
    /// (locations still generating) or already on the chapter's last page.
    var chapterPagesLeft: Int = 0

    /// `0...1` for the progress bar. Page numbers only exist once epub.js has
    /// finished its whole-book locations pass, so this is the reliable one.
    var fraction: Double { Double(pct) / 100 }

    var hasPageNumbers: Bool { totalPages > 0 }

    var chapterName: String? { chapterTitle.nilIfBlank }
}

/// A box on the page, in web-view coordinates — the same space the reader's
/// own overlay is laid out in, so a rect crosses the bridge ready to draw.
struct PageRect: Codable, Equatable, Hashable {
    var x: Double
    var y: Double
    var width: Double
    var height: Double

    var cgRect: CGRect {
        CGRect(x: x, y: y, width: width, height: height)
    }
}

/// Where one end of a selection sits: the caret box the host hangs a handle
/// off. `x` is the outer edge — the left of the first line, the right of the
/// last.
struct SelectionCaret: Codable, Equatable, Hashable {
    var x: Double
    var y: Double
    var height: Double
}

/// The live selection, as the glue reports it. One rect per visual line.
struct SelectionData: Codable, Equatable {
    /// Absent while a drag is in flight — deriving it costs a CFI walk the
    /// glue skips until the range settles.
    var cfiRange: String?
    var text: String
    var rects: [PageRect] = []
    var start: SelectionCaret?
    var end: SelectionCaret?
    /// A highlight this selection runs through, if any.
    var existing: String?
    /// True while the finger is still moving, so the host can hold the menu
    /// back until the range settles.
    var dragging: Bool = false
}

/// A tap on a highlight already on the page.
struct AnnotationTapData: Codable, Equatable {
    var cfiRange: String
    var rects: [PageRect] = []
}

/// Which end of a selection a handle drag is moving.
enum SelectionEdge: String {
    case start
    case end
}

/// A `WKWebView` with no edit menu of its own.
///
/// Selection is drawn by the app, not by WebKit — the glue disables WebKit's
/// own (its handles and loupe are laid out against a section iframe as wide as
/// the whole chapter, so they land in the wrong column). This is the belt to
/// that braces: even if a stray selection were ever made, no system callout
/// can appear over the reader's own menu.
final class AnnotatingWebView: WKWebView {
    override func canPerformAction(_ action: Selector, withSender sender: Any?) -> Bool {
        false
    }

    /// `super` first, then remove — the responder chain's contract. Removing
    /// ahead of it only takes out what is in the builder at that moment, and
    /// `WKWebView`'s own implementation puts its edit menu in afterwards.
    override func buildMenu(with builder: any UIMenuBuilder) {
        super.buildMenu(with: builder)
        builder.remove(menu: .standardEdit)
    }
}

/// Owns the web view and exposes a typed command surface to SwiftUI.
@Observable
@MainActor
final class ReaderController: NSObject {
    private(set) var isReady = false
    private(set) var failed = false
    /// What to say about a failure, when we know something more useful than
    /// "couldn't open".
    private(set) var failureMessage: String?

    /// Record that a resource the reader needs could not be served.
    ///
    /// The scheme handler is the only place that learns this: it hands the
    /// failure to WebKit and epub.js is left to notice, which for a book whose
    /// bytes never arrive it does not — the promise it would reject is never
    /// created, so nothing emitted `status: "error"` and the loading overlay
    /// stayed up for good. Reporting it directly from the handler is what makes
    /// the failure visible at all.
    func noteResourceFailure(_ message: String?) {
        guard !isReady else { return }
        failureMessage = message
        failed = true
    }
    private(set) var toc: [TocItem] = []
    private(set) var location: RelocateData?
    /// The live host-drawn selection. The glue owns the range; this is the
    /// geometry the reader draws it from.
    private(set) var selection: SelectionData?
    /// Set when a highlight already on the page is tapped; cleared by the host
    /// once it dismisses the menu.
    var tappedAnnotation: AnnotationTapData?

    /// Incremented on every centre tap inside the page. The glue owns the tap
    /// zones and swipe-to-turn; a SwiftUI overlay on top of the web view would
    /// swallow the touches those depend on.
    private(set) var chromeToggleToken = 0

    var settings: ReaderSettings {
        didSet {
            guard settings != oldValue else { return }
            settings.save()
            applySettings(changedFrom: oldValue)
        }
    }

    fileprivate var webView: WKWebView?
    private var book: Book?
    private var startCFI: String?
    private var pendingHighlights: [Highlight] = []
    /// What is currently drawn on the page, so a set arriving after first paint
    /// can be applied as a diff instead of a redraw.
    private var drawnHighlights: [Highlight] = []

    override init() {
        settings = ReaderSettings.load()
        super.init()
    }

    func configure(book: Book, startCFI: String?, highlights: [Highlight]) {
        self.book = book
        self.startCFI = startCFI
        pendingHighlights = highlights
    }

    // MARK: - Commands

    func next() { run("OmnibusReader.next()") }
    func previous() { run("OmnibusReader.prev()") }

    func display(_ target: String) {
        run("OmnibusReader.display(\(target.jsQuoted))")
    }

    /// Jump to a fraction (0–1) of the whole book, for the scrubber.
    func seek(toFraction fraction: Double) {
        run("OmnibusReader.seek(\(min(1, max(0, fraction))))")
    }

    /// The contents entry a fraction of the way through the book lands in.
    ///
    /// The last entry that starts at or before the target — entries arrive in
    /// reading order, and only carry positions once the locations pass has run,
    /// so unpositioned ones are skipped rather than guessed at.
    func tocEntry(atFraction fraction: Double) -> TocItem? {
        let target = Int((fraction * 100).rounded())
        var landed: TocItem?
        for item in toc {
            guard let pct = item.pct else { continue }
            if pct <= target { landed = item } else { break }
        }
        return landed
    }

    func addAnnotation(cfiRange: String, color: HighlightColor, hasNote: Bool = false) {
        run(
            "OmnibusReader.addAnnotation("
                + "\(cfiRange.jsQuoted), \(color.rawValue.jsQuoted), \(hasNote))"
        )
    }

    func removeAnnotation(cfiRange: String) {
        run("OmnibusReader.removeAnnotation(\(cfiRange.jsQuoted))")
    }

    /// Reconcile the drawn annotations against `items`, touching only what
    /// changed. Used when the server's set lands after the page is already up;
    /// before the reader is ready it just replaces the queue.
    func applyHighlights(_ items: [Highlight]) {
        guard isReady else {
            pendingHighlights = items
            return
        }
        // Kobo-origin rows have no CFI and are never drawn; only anchored
        // rows participate in the reconcile.
        let anchored = items.filter { $0.epubCFIRange != nil }
        let previous = Dictionary(
            drawnHighlights.compactMap { h in h.epubCFIRange.map { ($0, h) } },
            uniquingKeysWith: { _, latest in latest }
        )
        let next = Dictionary(
            anchored.compactMap { h in h.epubCFIRange.map { ($0, h) } },
            uniquingKeysWith: { _, latest in latest }
        )

        for range in previous.keys where next[range] == nil {
            removeAnnotation(cfiRange: range)
        }
        for (range, highlight) in next {
            let hasNote = highlight.note?.nilIfBlank != nil
            if let before = previous[range] {
                guard before.color != highlight.color
                    || (before.note?.nilIfBlank != nil) != hasNote
                else { continue }
                removeAnnotation(cfiRange: range)
            }
            addAnnotation(cfiRange: range, color: highlight.color, hasNote: hasNote)
        }
        drawnHighlights = anchored
    }

    // MARK: - Selection

    /// Pin the edge the reader is *not* dragging, so pulling one handle moves
    /// only that end of the range.
    func beginEdgeDrag(_ edge: SelectionEdge) {
        run("OmnibusReader.beginEdgeDrag(\(edge.rawValue.jsQuoted))")
    }

    /// Extend the selection to a point in web-view coordinates. Called on
    /// every handle-drag change; the glue coalesces to one layout read a frame.
    func dragEdge(to point: CGPoint) {
        run("OmnibusReader.extendSelectionTo(\(point.x), \(point.y))")
    }

    /// Settle a handle drag. This is the emit that carries the CFI and the
    /// overlapping highlight, both of which the in-flight emits leave out.
    func endEdgeDrag() {
        run("OmnibusReader.endSelectionDrag()")
    }

    func clearSelection() {
        selection = nil
        // The range lives in the glue, so dropping our copy isn't enough —
        // left standing, the next tap on the page would only dismiss it.
        run("OmnibusReader.clearSelection()")
    }

    func teardown() {
        run("OmnibusReader.destroy()")
        webView?.stopLoading()
        webView = nil
    }

    private func applySettings(changedFrom old: ReaderSettings) {
        guard isReady else { return }
        if settings.fontSize != old.fontSize {
            run("OmnibusReader.setFontSize(\(settings.fontSize))")
        }
        if settings.theme != old.theme {
            run("OmnibusReader.setTheme(\(settings.theme.jsQuoted))")
        }
        if settings.fontFamily != old.fontFamily {
            run("OmnibusReader.setFont(\(settings.fontFamily.jsQuoted))")
        }
        if settings.lineHeight != old.lineHeight {
            run("OmnibusReader.setLineHeight(\(settings.lineHeight))")
        }
        if settings.margins != old.margins {
            run("OmnibusReader.setMargins(\(settings.margins.css.jsQuoted))")
        }
        if settings.justify != old.justify {
            run("OmnibusReader.setJustify(\(settings.justify))")
        }
    }

    fileprivate func run(_ script: String) {
        webView?.evaluateJavaScript(script)
    }

    fileprivate func bootReader() {
        guard let book else { return }
        var options: [String: Any] = [
            "theme": settings.theme,
            "fontSize": settings.fontSize,
            "fontFamily": settings.fontFamily,
            "lineHeight": settings.lineHeight,
            "maxWidth": settings.margins.css,
            "justify": settings.justify,
            // Without allow-scripts on the section iframe WebKit dispatches no
            // events into it — selection and gestures are dead on iOS.
            "allowScriptedContent": true,
            "locationsKey": book.uuid,
        ]
        if let startCFI { options["cfi"] = startCFI }

        let json = (try? JSONSerialization.data(withJSONObject: options))
            .flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
        let fileURL = "omnibus-reader://book/\(book.uuid).epub"
        run("OmnibusReader.init('stage', \(fileURL.jsQuoted), \(json))")
    }

    fileprivate func handle(message: [String: Any]) {
        guard let type = message["type"] as? String else { return }
        let payload = message["payload"] as? String

        switch type {
        case "hostReady":
            bootReader()

        case "status":
            switch payload {
            case "ready":
                isReady = true
                failed = false
                for highlight in pendingHighlights {
                    guard let cfiRange = highlight.epubCFIRange else { continue }
                    addAnnotation(
                        cfiRange: cfiRange,
                        color: highlight.color,
                        hasNote: highlight.note?.nilIfBlank != nil
                    )
                }
                drawnHighlights = pendingHighlights.filter { $0.epubCFIRange != nil }
                pendingHighlights = []
            case "error":
                failed = true
            default:
                break
            }

        case "relocate":
            guard let payload, let data = payload.data(using: .utf8),
                  let decoded = try? JSONDecoder().decode(RelocateData.self, from: data)
            else { return }
            location = decoded

        case "toc":
            guard let payload, let data = payload.data(using: .utf8),
                  let decoded = try? JSONDecoder().decode([TocItem].self, from: data)
            else { return }
            toc = decoded

        case "selection":
            guard let payload, let data = payload.data(using: .utf8),
                  let decoded = try? JSONDecoder().decode(SelectionData.self, from: data)
            else { return }
            // A live selection and a tapped highlight are mutually exclusive
            // menus; the newer one wins.
            tappedAnnotation = nil
            let previous = selection
            selection = decoded
            // The whole point of word snapping is that you can feel it: a tick
            // per word crossed is what tells a finger it landed on a boundary
            // without having to look at what is under it.
            if previous == nil {
                Haptics.tap()
            } else if previous?.text != decoded.text {
                Haptics.select()
            }

        case "selectionCleared":
            selection = nil

        case "annotationTap":
            guard let payload, let data = payload.data(using: .utf8),
                  let decoded = try? JSONDecoder().decode(AnnotationTapData.self, from: data)
            else { return }
            selection = nil
            tappedAnnotation = decoded

        case "toggleChrome":
            chromeToggleToken &+= 1

        case "shareText":
            if let payload { ShareSheet.present(items: [payload]) }

        case "shareImage":
            guard let payload, let data = payload.data(using: .utf8),
                  let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let dataURL = object["dataUrl"] as? String,
                  let comma = dataURL.firstIndex(of: ","),
                  let imageData = Data(base64Encoded: String(dataURL[dataURL.index(after: comma)...])),
                  let image = UIImage(data: imageData)
            else { return }
            ShareSheet.present(items: [image])

        default:
            break
        }
    }
}

// MARK: - UIViewRepresentable

struct ReaderWebView: UIViewRepresentable {
    let controller: ReaderController
    let bookUUID: String

    func makeCoordinator() -> Coordinator {
        Coordinator(controller: controller, bookUUID: bookUUID)
    }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.setURLSchemeHandler(context.coordinator, forURLScheme: Self.scheme)
        configuration.userContentController.add(context.coordinator, name: "omnibus")
        configuration.allowsInlineMediaPlayback = true
        configuration.suppressesIncrementalRendering = false

        let webView = AnnotatingWebView(frame: .zero, configuration: configuration)
        webView.isOpaque = false
        webView.backgroundColor = .clear
        webView.scrollView.isScrollEnabled = false
        webView.scrollView.bounces = false
        webView.scrollView.contentInsetAdjustmentBehavior = .never
        // The glue owns page turns; the web view's own back/forward swipe would
        // fight it.
        webView.allowsBackForwardNavigationGestures = false

        controller.webView = webView

        guard let url = URL(string: "\(Self.scheme)://app/reader.html") else { return webView }
        webView.load(URLRequest(url: url))
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {}

    static func dismantleUIView(_ webView: WKWebView, coordinator: Coordinator) {
        webView.configuration.userContentController.removeScriptMessageHandler(forName: "omnibus")
    }

    static let scheme = "omnibus-reader"

    /// Serves the bundled reader assets and the book bytes. Using a custom
    /// scheme (rather than `file://` plus a private preference) keeps
    /// everything on public API and gives epub.js one same-origin space.
    @MainActor
    final class Coordinator: NSObject, WKScriptMessageHandler, WKURLSchemeHandler {
        private let controller: ReaderController
        private let bookUUID: String
        private var tasks: [ObjectIdentifier: Task<Void, Never>] = [:]

        init(controller: ReaderController, bookUUID: String) {
            self.controller = controller
            self.bookUUID = bookUUID
        }

        func userContentController(
            _ controller: WKUserContentController, didReceive message: WKScriptMessage
        ) {
            guard let body = message.body as? [String: Any] else { return }
            self.controller.handle(message: body)
        }

        func webView(_ webView: WKWebView, start urlSchemeTask: any WKURLSchemeTask) {
            guard let url = urlSchemeTask.request.url else {
                urlSchemeTask.didFailWithError(URLError(.badURL))
                return
            }

            let task = Task { [weak self] in
                guard let self else { return }
                do {
                    let (data, mime) = try await self.payload(for: url)
                    guard !Task.isCancelled else { return }
                    let response = HTTPURLResponse(
                        url: url,
                        statusCode: 200,
                        httpVersion: "HTTP/1.1",
                        headerFields: [
                            "Content-Type": mime,
                            "Content-Length": String(data.count),
                            "Access-Control-Allow-Origin": "*",
                        ]
                    )!
                    urlSchemeTask.didReceive(response)
                    urlSchemeTask.didReceive(data)
                    urlSchemeTask.didFinish()
                } catch {
                    guard !Task.isCancelled else { return }
                    urlSchemeTask.didFailWithError(error)
                    self.controller.noteResourceFailure(
                        (error as? APIError)?.isRecoverableOffline == true
                            ? "This book isn't downloaded, and the server can't be reached."
                            : nil
                    )
                }
            }
            tasks[ObjectIdentifier(urlSchemeTask)] = task
        }

        func webView(_ webView: WKWebView, stop urlSchemeTask: any WKURLSchemeTask) {
            let key = ObjectIdentifier(urlSchemeTask)
            tasks[key]?.cancel()
            tasks[key] = nil
        }

        /// `omnibus-reader://app/<asset>` → bundle; `omnibus-reader://book/<uuid>.epub`
        /// → the downloaded file, else a bearer-authenticated fetch.
        private func payload(for url: URL) async throws -> (Data, String) {
            let name = url.lastPathComponent

            if url.host == "book" {
                if let local = DownloadManager.shared.localURL(for: bookUUID, kind: .ebook),
                   let data = try? Data(contentsOf: local) {
                    return (data, "application/epub+zip")
                }
                let data = try await APIClient.shared.data(for: "/api/ebooks/\(bookUUID)/file")
                return (data, "application/epub+zip")
            }

            guard let assetURL = Bundle.main.url(forResource: name, withExtension: nil)
                ?? Bundle.main.url(
                    forResource: (name as NSString).deletingPathExtension,
                    withExtension: (name as NSString).pathExtension,
                    subdirectory: "Web"
                ),
                let data = try? Data(contentsOf: assetURL)
            else { throw URLError(.fileDoesNotExist) }

            return (data, Self.mime(for: name))
        }

        private static func mime(for name: String) -> String {
            if name.hasSuffix(".js") { return "text/javascript; charset=utf-8" }
            if name.hasSuffix(".css") { return "text/css; charset=utf-8" }
            if name.hasSuffix(".html") { return "text/html; charset=utf-8" }
            return "application/octet-stream"
        }
    }
}

private extension String {
    /// JSON-quotes a string for safe interpolation into an evaluated script.
    var jsQuoted: String {
        guard let data = try? JSONSerialization.data(
            withJSONObject: self, options: [.fragmentsAllowed]
        ), let quoted = String(data: data, encoding: .utf8) else { return "\"\"" }
        return quoted
    }
}
