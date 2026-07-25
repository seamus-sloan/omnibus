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
            ReaderView(book: session.book)
        }
        .fullScreenCover(item: playerBinding) { session in
            PlayerView(book: session.book)
        }
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

    func openReader(_ book: Book) {
        player = nil
        reader = ReaderSession(book: book)
    }

    func openPlayer(_ book: Book) {
        reader = nil
        player = ReaderSession(book: book)
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
