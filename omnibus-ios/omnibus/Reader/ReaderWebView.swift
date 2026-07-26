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

struct SelectionData: Codable {
    var cfiRange: String
    var text: String
    var rect: SelectionRect?
    /// A highlight this selection runs through, if any.
    var existing: String?

    struct SelectionRect: Codable {
        var x: Double
        var y: Double
        var width: Double
        var height: Double?
    }
}

/// A tap on a highlight already on the page.
struct AnnotationTapData: Codable {
    var cfiRange: String
    var rect: SelectionData.SelectionRect?
}

/// What the reader was asked to do from the system's own selection menu.
enum SelectionMenuAction: Equatable {
    case highlight
    case note
    case removeHighlight
}

/// A `WKWebView` that contributes Omnibus's verbs to the system edit menu.
///
/// Selecting text raises iOS's own callout (Copy, Look Up, Translate, Share),
/// and our floating colour bar was a second, competing menu over the top of it
/// — two answers to one gesture. Apple Books instead *joins* that menu, so
/// Highlight and Note sit alongside Copy and the selection behaves like text
/// everywhere else on the system. `buildMenu(with:)` is how a responder adds
/// to it; `UIMenuController`, the old way, was deprecated in iOS 16.
final class AnnotatingWebView: WKWebView {
    /// Kept in sync with the glue's selection events so the menu only offers
    /// these verbs when there is prose selected to apply them to.
    var hasSelection = false
    var selectionHasHighlight = false
    var onMenuAction: ((SelectionMenuAction) -> Void)?

    override func buildMenu(with builder: any UIMenuBuilder) {
        super.buildMenu(with: builder)
        guard hasSelection else { return }

        var actions: [UIAction] = [
            UIAction(title: "Highlight", image: UIImage(systemName: "highlighter")) {
                [weak self] _ in self?.onMenuAction?(.highlight)
            },
            UIAction(title: "Note", image: UIImage(systemName: "square.and.pencil")) {
                [weak self] _ in self?.onMenuAction?(.note)
            },
        ]
        if selectionHasHighlight {
            actions.append(
                UIAction(
                    title: "Remove Highlight",
                    image: UIImage(systemName: "trash"),
                    attributes: .destructive
                ) { [weak self] _ in self?.onMenuAction?(.removeHighlight) }
            )
        }

        builder.insertChild(
            UIMenu(title: "", options: .displayInline, children: actions),
            atStartOfMenu: .standardEdit
        )
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
    private(set) var selection: SelectionData? {
        didSet { syncSelectionMenuState() }
    }
    /// Set when a highlight already on the page is tapped; cleared by the host
    /// once it dismisses the menu.
    var tappedAnnotation: AnnotationTapData?
    /// Bumped when the system edit menu invokes one of our verbs, so the view
    /// can act on it with the selection that was live at the time.
    private(set) var menuAction: SelectionMenuAction?
    private(set) var menuActionToken = 0

    fileprivate func noteMenuAction(_ action: SelectionMenuAction) {
        menuAction = action
        menuActionToken &+= 1
    }

    /// The menu is built by UIKit on demand, so the web view needs the current
    /// selection state cached rather than asking for it mid-build.
    private func syncSelectionMenuState() {
        guard let annotating = webView as? AnnotatingWebView else { return }
        annotating.hasSelection = selection != nil
        annotating.selectionHasHighlight = selection?.existing != nil
    }

    /// Incremented on every centre tap inside the page. The glue owns the tap
    /// zones and swipe-to-turn; a SwiftUI overlay on top of the web view would
    /// swallow the touches those depend on.
    private(set) var chromeToggleToken = 0

    /// When a tap on a highlight last arrived.
    ///
    /// One tap on a mark reaches us twice — as an annotation tap from the
    /// mark's own listener and as a page tap from the document's. Which lands
    /// first depends on DOM listener order (target phase before bubble), so the
    /// two are paired by arrival time rather than by sequence. Without this the
    /// bars flip underneath the menu as it opens, and putting them back costs
    /// the reader an extra tap after closing it.
    private var lastAnnotationTapAt = Date.distantPast
    private static let tapPairWindow: TimeInterval = 0.3

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
        let previous = Dictionary(
            drawnHighlights.map { ($0.epubCFIRange, $0) }, uniquingKeysWith: { _, latest in latest }
        )
        let next = Dictionary(
            items.map { ($0.epubCFIRange, $0) }, uniquingKeysWith: { _, latest in latest }
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
        drawnHighlights = items
    }

    func clearSelection() {
        selection = nil
        // Has to go through the glue: the selection lives in the section
        // iframe, which the host window's `getSelection` can't reach.
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
                    addAnnotation(
                        cfiRange: highlight.epubCFIRange,
                        color: highlight.color,
                        hasNote: highlight.note?.nilIfBlank != nil
                    )
                }
                drawnHighlights = pendingHighlights
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
            selection = decoded

        case "selectionCleared":
            selection = nil

        case "annotationTap":
            guard let payload, let data = payload.data(using: .utf8),
                  let decoded = try? JSONDecoder().decode(AnnotationTapData.self, from: data)
            else { return }
            // A tap on a highlight and a live text selection are mutually
            // exclusive menus; the newer one wins.
            selection = nil
            lastAnnotationTapAt = Date()
            tappedAnnotation = decoded

        case "toggleChrome":
            // The same tap that opened an annotation menu, arriving a second
            // time as a page tap.
            guard Date().timeIntervalSince(lastAnnotationTapAt) > Self.tapPairWindow
            else { break }
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
        webView.onMenuAction = { [controller] action in
            controller.noteMenuAction(action)
        }
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
