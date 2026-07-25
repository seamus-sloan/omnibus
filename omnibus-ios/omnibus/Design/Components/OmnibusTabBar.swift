//  OmnibusTabBar.swift
//  Custom bottom navigation.
//
//  Replaces the iOS 26 floating pill bar with a flush, full-width bar in the
//  Airbnb mould: a hairline rule, outline icons that fill on selection, and a
//  tight label under each. The native bar is hidden rather than restyled —
//  `TabView` still owns tab state and lazy loading, so scroll position and
//  in-flight loads survive switching.

import SwiftUI

enum AppTab: Int, CaseIterable, Hashable {
    case library, search, stats, you

    var label: String {
        switch self {
        case .library: "Library"
        case .search: "Search"
        case .stats: "Stats"
        case .you: "You"
        }
    }

    /// Outline for the resting state, filled for the selected one.
    var icon: String {
        switch self {
        case .library: "books.vertical"
        case .search: "magnifyingglass"
        case .stats: "chart.bar"
        case .you: "person.crop.circle"
        }
    }

    /// `magnifyingglass` has no filled counterpart; it takes weight instead.
    var selectedIcon: String {
        self == .search ? icon : icon + ".fill"
    }

    var selectedWeight: Font.Weight {
        self == .search ? .bold : .regular
    }
}

struct OmnibusTabBar: View {
    @Binding var selection: AppTab
    /// Called when the already-selected tab is tapped again.
    var onReselect: (AppTab) -> Void = { _ in }

    @Environment(\.palette) private var palette

    var body: some View {
        HStack(spacing: 0) {
            ForEach(AppTab.allCases, id: \.self) { tab in
                item(tab)
            }
        }
        .padding(.top, 9)
        .padding(.bottom, 6)
        .overlay(alignment: .top) {
            Rectangle()
                .fill(palette.line2.color)
                .frame(height: 0.5)
        }
        // `ignoresSafeAreaEdges` extends the fill under the home indicator
        // while leaving the icons and labels inset above it. Putting
        // `.ignoresSafeArea()` on the background view instead drags the labels
        // down into the indicator.
        .background(palette.bg0Color.opacity(0.55), ignoresSafeAreaEdges: .bottom)
        .background(.ultraThinMaterial, ignoresSafeAreaEdges: .bottom)
    }

    private func item(_ tab: AppTab) -> some View {
        let isOn = selection == tab

        return Button {
            guard selection != tab else {
                onReselect(tab)
                return
            }
            Haptics.select()
            selection = tab
        } label: {
            VStack(spacing: 4) {
                Image(systemName: isOn ? tab.selectedIcon : tab.icon)
                    .font(.system(size: 21, weight: isOn ? tab.selectedWeight : .regular))
                    .symbolRenderingMode(.monochrome)
                    .frame(height: 24)
                    // A gentle pop on selection — the whole cue is otherwise
                    // just a color change, which reads as sluggish.
                    .scaleEffect(isOn ? 1.06 : 1)
                    // The outline→filled swap is a glyph substitution, so it
                    // cuts rather than animates; the bounce is what carries it.
                    .symbolEffect(.bounce, options: .speed(1.5), value: isOn)

                Text(tab.label)
                    .font(.ui(10.5, weight: isOn ? .semibold : .medium))
            }
            .foregroundStyle(isOn ? palette.accentColor : palette.ink3Color)
            .frame(maxWidth: .infinity)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .animation(Motion.snap, value: isOn)
        .accessibilityLabel(tab.label)
        .accessibilityAddTraits(isOn ? [.isSelected, .isButton] : .isButton)
    }
}

/// Horizontal category strip under the search pill: icon over label with an
/// underline on the active entry.
struct CategoryStrip<Category: Hashable>: View {
    struct Item: Identifiable {
        let id: Category
        let label: String
        let icon: String
    }

    let items: [Item]
    @Binding var selection: Category

    @Environment(\.palette) private var palette
    @Namespace private var underline

    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 26) {
                ForEach(items) { item in
                    entry(item)
                }
            }
            .padding(.horizontal, Spacing.screen)
        }
        .scrollIndicators(.hidden)
    }

    private func entry(_ item: Item) -> some View {
        let isOn = selection == item.id

        return Button {
            guard selection != item.id else { return }
            Haptics.select()
            withAnimation(Motion.snap) {
                selection = item.id
            }
        } label: {
            VStack(spacing: 6) {
                Image(systemName: item.icon)
                    .font(.system(size: 19, weight: .regular))
                    .frame(height: 22)

                Text(item.label)
                    .font(.ui(11.5, weight: isOn ? .semibold : .regular))

                // The indicator is always laid out and only changes color, so
                // selection never shifts the strip's height.
                Capsule()
                    .fill(isOn ? palette.ink0Color : .clear)
                    .frame(height: 2)
            }
            .foregroundStyle(isOn ? palette.ink0Color : palette.ink3Color)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}
