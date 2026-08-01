//  MainTabView.swift
//  The signed-in shell: five native tabs, one NavigationStack each (so every
//  push gets interactive swipe-back for free), and the audiobook mini player
//  docked in the tab bar's accessory slot.

import SwiftUI

/// Type-safe navigation targets. One enum shared by every stack so a link from
/// the search tab and one from the library tab push the same screen.
enum Destination: Hashable {
    case book(uuid: String)
    case author(id: Int64)
    case series(id: Int64)
    case seriesIndex
    case shelves
    case shelf(id: Int64)
    case tags
    case tag(name: String)
    case settings
    case downloads
    case authorsIndex
    case searchResults(query: String)
    case metadataEdit(uuid: String)
}

struct MainTabView: View {
    @Environment(AppState.self) private var app
    @Environment(AudioPlayer.self) private var player
    @Environment(\.palette) private var palette

    @State private var selection: AppTab = .library
    @State private var addSheetPresented = false
    @State private var reselect = TabReselect()

    var body: some View {
        TabView(selection: $selection) {
            Tab(value: AppTab.library) {
                LibraryView(addSheetPresented: $addSheetPresented)
            }
            Tab(value: AppTab.search) {
                SearchTab()
            }
            Tab(value: AppTab.stats) {
                StatsView()
            }
            Tab(value: AppTab.you) {
                AccountView()
            }
        }
        // `TabView` keeps tab state and lazy loading; only its chrome is
        // replaced. Hiding the bar and insetting our own is what lets the mini
        // player sit directly above the tabs as one block.
        .toolbar(.hidden, for: .tabBar)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 0) {
                if player.isActive {
                    MiniPlayerBar()
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
                OmnibusTabBar(selection: $selection) { tab in
                    reselect.fire(tab)
                }
            }
            .animation(Motion.glide, value: player.isActive)
        }
        .environment(reselect)
        .sheet(isPresented: $addSheetPresented) {
            AddBooksSheet()
        }
        .fullScreenCover(item: readerBinding) { session in
            // Comic-only books get the native pager; everything else the
            // epub.js host. A book carrying both formats keeps the EPUB as
            // its primary read, matching the web pager and the server's
            // `/file` resolution.
            if session.book.opensAsComic {
                ComicReaderView(book: session.book)
            } else {
                ReaderView(book: session.book)
            }
        }
        .fullScreenCover(item: playerBinding) { session in
            PlayerView(book: session.book, fileID: session.fileID)
        }
        // Presented here rather than at each Read/Listen button: the surfaces
        // being refused live above the tabs, and every entry point into them
        // goes through `Presentation`.
        .alert(item: unavailableBinding) { unavailable in
            Alert(
                title: Text(unavailable.heading),
                message: Text(unavailable.message),
                dismissButton: .default(Text("OK"))
            )
        }
    }

    private var unavailableBinding: Binding<UnavailableBook?> {
        Binding(
            get: { Presentation.shared.unavailable },
            set: { Presentation.shared.unavailable = $0 }
        )
    }

    // The reader and the full-screen player are app-global presentations —
    // they can be launched from any tab, so they live above the TabView.
    private var readerBinding: Binding<ReaderSession?> {
        Binding(
            get: { Presentation.shared.reader },
            set: { Presentation.shared.reader = $0 }
        )
    }

    private var playerBinding: Binding<ReaderSession?> {
        Binding(
            get: { Presentation.shared.player },
            set: { Presentation.shared.player = $0 }
        )
    }
}

/// Broadcasts "the tab you're already on was tapped again".
///
/// Tapping the current tab is the standard iOS gesture for returning to that
/// tab's root; the bar previously swallowed it, stranding you in a pushed
/// screen with no way back but the back button.
@Observable
@MainActor
final class TabReselect {
    private(set) var token = 0
    private(set) var tab: AppTab?

    func fire(_ tab: AppTab) {
        self.tab = tab
        token &+= 1
    }
}

/// A book opened into an immersive surface.
struct ReaderSession: Identifiable, Equatable {
    let book: Book
    /// The audiobook file to open, when a specific one was chosen or
    /// resumed. `nil` lets the player resolve it (saved position, then the
    /// server default). Reader sessions never set it.
    var fileID: Int64?
    var id: String { book.uuid }
}

/// Global presentation state for the two full-screen surfaces. Kept out of
/// `AppState` so a reader open doesn't invalidate every view observing auth.
@Observable
@MainActor
final class Presentation {
    static let shared = Presentation()

    var reader: ReaderSession?
    var player: ReaderSession?

    /// Bumped once a reading or listening session has *persisted* its final
    /// position. Surfaces that show resume state observe this instead of
    /// reacting to dismissal — the reader writes its last position from an
    /// `onDisappear` task, so anything keyed on the dismissal itself refreshes
    /// before the write lands and reads back the previous book.
    private(set) var progressToken = 0

    func noteProgressPersisted() {
        progressToken &+= 1
    }

    /// A book the reader or player was asked for and can't have. Presented as
    /// an alert rather than opened, so the answer arrives on the tap instead of
    /// as a spinner that never resolves.
    var unavailable: UnavailableBook?

    func openReader(_ book: Book) {
        guard canOpen(book, kind: .ebook) else {
            unavailable = UnavailableBook(title: book.displayTitle, kind: .ebook)
            return
        }
        player = nil
        reader = ReaderSession(book: book)
    }

    func openPlayer(_ book: Book, fileID: Int64? = nil) {
        guard canOpen(book, kind: .audio) else {
            unavailable = UnavailableBook(title: book.displayTitle, kind: .audio)
            return
        }
        reader = nil
        player = ReaderSession(book: book, fileID: fileID)
    }

    /// Whether this format can actually be opened: the file is on the device,
    /// or the server is there to stream it from.
    ///
    /// Only the file settles it — a book is streamed, not cached, so having
    /// read it before means nothing once the server is gone.
    private func canOpen(_ book: Book, kind: DownloadKind) -> Bool {
        DownloadManager.shared.isDownloaded(book.uuid, kind: kind) || Connectivity.shared.isOnline
    }
}

/// A book that can't be opened, and why.
struct UnavailableBook: Identifiable, Equatable {
    let title: String
    let kind: DownloadKind

    var id: String { "\(title):\(kind.rawValue)" }

    var heading: String {
        kind == .audio ? "Can't play offline" : "Can't read offline"
    }

    var message: String {
        let noun = kind == .audio ? "audiobook" : "book"
        return """
            “\(title)” isn't downloaded to this device, and the server can't be \
            reached. Connect to the internet to stream it, or download the \
            \(noun) while you're online to read it anywhere.
            """
    }
}

/// Routes a `Destination` to its screen. Attached once per NavigationStack.
struct DestinationRouter: ViewModifier {
    func body(content: Content) -> some View {
        content.navigationDestination(for: Destination.self) { destination in
            switch destination {
            case let .book(uuid):
                BookDetailView(uuid: uuid)
            case let .author(id):
                AuthorDetailView(id: id)
            case let .series(id):
                SeriesDetailView(id: id)
            case .seriesIndex:
                SeriesIndexView()
            case .shelves:
                ShelvesView()
            case let .shelf(id):
                ShelfDetailView(id: id)
            case .tags:
                TagCloudView()
            case let .tag(name):
                SearchResultsView(query: name, title: name)
            case .settings:
                SettingsView()
            case .downloads:
                DownloadsView()
            case .authorsIndex:
                AuthorsView()
            case let .searchResults(query):
                SearchResultsView(query: query, title: "“\(query)”")
            case let .metadataEdit(uuid):
                MetadataEditView(uuid: uuid)
            }
        }
    }
}

extension View {
    func withDestinations() -> some View {
        modifier(DestinationRouter())
    }
}
