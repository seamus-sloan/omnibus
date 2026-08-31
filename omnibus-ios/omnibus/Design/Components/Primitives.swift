//  Primitives.swift
//  Shared building blocks: buttons, chips, section headers, state views,
//  haptics, and the offline pill.

import SwiftUI
import UIKit

// MARK: - Haptics

enum Haptics {
    @MainActor static func tap() {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
    }

    @MainActor static func select() {
        UISelectionFeedbackGenerator().selectionChanged()
    }

    @MainActor static func success() {
        UINotificationFeedbackGenerator().notificationOccurred(.success)
    }

    @MainActor static func warning() {
        UINotificationFeedbackGenerator().notificationOccurred(.warning)
    }
}

// MARK: - Buttons

/// Primary filled action. Scales slightly on press — the single most
/// recognisable "this is a native control" cue.
struct FilledButtonStyle: ButtonStyle {
    @Environment(\.palette) private var palette
    var prominent = true

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.ui(16, weight: .semibold))
            .foregroundStyle(prominent ? palette.accentInk.color : palette.ink0Color)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 14)
            .background(
                RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                    .fill(prominent ? palette.accentColor : palette.bg2Color)
            )
            .scaleEffect(configuration.isPressed ? 0.97 : 1)
            .opacity(configuration.isPressed ? 0.9 : 1)
            .animation(Motion.lift, value: configuration.isPressed)
    }
}

struct QuietButtonStyle: ButtonStyle {
    @Environment(\.palette) private var palette

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.ui(15, weight: .medium))
            .foregroundStyle(palette.ink1Color)
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .background(
                RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                    .fill(palette.bg2Color.opacity(configuration.isPressed ? 1 : 0.7))
            )
            .scaleEffect(configuration.isPressed ? 0.97 : 1)
            .animation(Motion.lift, value: configuration.isPressed)
    }
}

/// Row-level tap feedback for cards and list rows.
struct PressableStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.975 : 1)
            .opacity(configuration.isPressed ? 0.85 : 1)
            .animation(Motion.lift, value: configuration.isPressed)
    }
}

// MARK: - Chips

struct Chip: View {
    let label: String
    var isOn = false
    var systemImage: String?
    /// Rendered in the dimmer ink so the number reads as an annotation on the
    /// label rather than as part of it.
    var count: Int?

    @Environment(\.palette) private var palette

    var body: some View {
        HStack(spacing: 5) {
            if let systemImage {
                Image(systemName: systemImage).font(.system(size: 11, weight: .semibold))
            }
            Text(label)
            if let count {
                Text("\(count)")
                    .font(.monoUI(11))
                    .foregroundStyle(isOn ? palette.accentInk.color.opacity(0.7) : palette.ink3Color)
            }
        }
        .font(.ui(13, weight: .medium))
        .foregroundStyle(isOn ? palette.accentInk.color : palette.ink1Color)
        .padding(.horizontal, 11)
        .padding(.vertical, 6)
        .background(
            Capsule().fill(isOn ? palette.accentColor : palette.bg2Color)
        )
        .overlay(
            Capsule().strokeBorder(isOn ? .clear : palette.line2.color, lineWidth: 0.5)
        )
    }
}

/// A small metadata badge — format tags, physical-copy marker.
struct Badge: View {
    let text: String
    var tint: Color?

    @Environment(\.palette) private var palette

    var body: some View {
        Text(text.uppercased())
            .font(.monoUI(9, weight: .semibold))
            .tracking(0.6)
            .foregroundStyle(tint ?? palette.ink2Color)
            .padding(.horizontal, 5)
            .padding(.vertical, 2.5)
            .background(
                RoundedRectangle(cornerRadius: 4, style: .continuous)
                    .fill(palette.bg3Color.opacity(0.85))
            )
    }
}

// MARK: - Section header

struct SectionHeader<Trailing: View>: View {
    let title: String
    var subtitle: String?
    @ViewBuilder var trailing: () -> Trailing

    @Environment(\.palette) private var palette

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.display(26))
                    .foregroundStyle(palette.ink0Color)
                if let subtitle {
                    Text(subtitle)
                        .font(.ui(13))
                        .foregroundStyle(palette.ink2Color)
                }
            }
            Spacer(minLength: Spacing.sm)
            trailing()
        }
    }
}

extension SectionHeader where Trailing == EmptyView {
    init(title: String, subtitle: String? = nil) {
        self.init(title: title, subtitle: subtitle, trailing: { EmptyView() })
    }
}

// MARK: - State views

struct LoadingView: View {
    var label: String?
    @Environment(\.palette) private var palette

    var body: some View {
        VStack(spacing: Spacing.md) {
            ProgressView()
                .controlSize(.large)
                .tint(palette.ink2Color)
            if let label {
                Text(label)
                    .font(.ui(14))
                    .foregroundStyle(palette.ink2Color)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(.vertical, 60)
    }
}

/// A book-shaped plate with nothing on it — the motif every "nothing here
/// yet" surface leads with.
///
/// This app's world is 2:3 covers, so an empty surface says so with an empty
/// cover: the same dashed plate the new-shelf tile draws, at rest. A bare SF
/// Symbol floating on the ground was the one stock-iOS element left on these
/// screens, and it read as a missing image rather than as a considered state.
struct GhostPlate: View {
    let glyph: String
    var width: CGFloat = 74
    /// Two dimmer plates fanned behind the lead one. For surfaces that hold a
    /// *collection* — a shelf, a library, a download list — where one plate
    /// under-describes what is missing.
    var fanned = false

    @Environment(\.palette) private var palette

    private var height: CGFloat { width * 1.5 }

    var body: some View {
        ZStack {
            if fanned {
                sibling(angle: -11, x: -width * 0.5)
                sibling(angle: 11, x: width * 0.5)
            }
            lead
        }
        // Bound the fan explicitly: the rotated siblings overflow the lead
        // plate, and an unbounded ZStack would let them run under whatever
        // sits beside it.
        .frame(width: fanned ? width * 2.2 : width, height: height + 16)
        .accessibilityHidden(true)
    }

    /// The plate in front. Its ground is opaque — a translucent one let the
    /// fan behind it show straight through, so three plates read as one
    /// tangle of lines rather than as a stack.
    private var lead: some View {
        RoundedRectangle(cornerRadius: Radius.sm, style: .continuous)
            .fill(palette.bg0Color)
            .overlay(
                RoundedRectangle(cornerRadius: Radius.sm, style: .continuous)
                    .fill(palette.bg1Color.opacity(0.55))
            )
            .overlay(
                RoundedRectangle(cornerRadius: Radius.sm, style: .continuous)
                    .strokeBorder(
                        palette.line.color,
                        style: StrokeStyle(lineWidth: 1, dash: [5, 4])
                    )
            )
            .overlay {
                Image(systemName: glyph)
                    .font(.system(size: width * 0.3, weight: .light))
                    .foregroundStyle(palette.ink3Color)
            }
            .frame(width: width, height: height)
    }

    /// One of the plates fanned behind: outline only, and dropped a little so
    /// the lead plate reads as the nearest of a stack rather than as the
    /// middle of a row.
    private func sibling(angle: Double, x: CGFloat) -> some View {
        RoundedRectangle(cornerRadius: Radius.sm, style: .continuous)
            .fill(palette.bg0Color)
            .overlay(
                RoundedRectangle(cornerRadius: Radius.sm, style: .continuous)
                    .strokeBorder(palette.line2.color, lineWidth: 1)
            )
            .frame(width: width, height: height)
            .rotationEffect(.degrees(angle), anchor: .bottom)
            .offset(x: x, y: 6)
    }
}

/// The gentle call to action an empty state offers — the accent-tinted capsule
/// the book detail's WRITE pill established, rather than a grey slab that reads
/// as a form control.
struct EmptyStateActionStyle: ButtonStyle {
    @Environment(\.palette) private var palette

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.ui(14, weight: .medium))
            .foregroundStyle(palette.accentColor)
            .padding(.horizontal, 18)
            .frame(height: 38)
            .background(Capsule().fill(palette.accentColor.opacity(0.14)))
            .overlay(Capsule().strokeBorder(palette.accentColor.opacity(0.45), lineWidth: 0.5))
            .scaleEffect(configuration.isPressed ? 0.97 : 1)
            .animation(Motion.lift, value: configuration.isPressed)
    }
}

/// A surface with nothing on it yet.
///
/// Set as a page rather than a notice: the ghost plate, an accent rule, an
/// optional mono kicker, the headline in the display cut, and the explanation
/// under it. It claims the whole container — that is not cosmetic. Callers
/// wrap it in a `Group` carrying `.background(ScreenBackground())`, and a
/// `Group` is only as tall as its content, so a state view that sized to its
/// text painted the page ground as a *band* across an otherwise system-black
/// screen.
struct EmptyStateView: View {
    let icon: String
    let title: String
    var message: String?
    /// Mono small-caps lead-in above the headline, in the voice the rest of
    /// the app uses for section kickers.
    var kicker: String?
    /// Fans two dimmer plates behind the motif — for surfaces that hold a
    /// collection rather than a single thing.
    var fanned = false
    var actionTitle: String?
    var action: (() -> Void)?

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(spacing: 0) {
            GhostPlate(glyph: icon, fanned: fanned)

            Rectangle()
                .fill(palette.accentColor)
                .frame(width: 26, height: 1.5)
                .padding(.top, 26)

            if let kicker {
                Text(kicker.uppercased())
                    .font(.monoUI(9.5))
                    .tracking(1.6)
                    .foregroundStyle(palette.ink3Color)
                    .padding(.top, 14)
            }

            Text(title)
                .font(.display(29))
                .foregroundStyle(palette.ink0Color)
                .multilineTextAlignment(.center)
                .padding(.top, kicker == nil ? 14 : 6)

            if let message {
                Text(message)
                    .font(.ui(13.5))
                    .foregroundStyle(palette.ink2Color)
                    .multilineTextAlignment(.center)
                    .lineSpacing(3)
                    .frame(maxWidth: 300)
                    .padding(.top, 8)
            }

            if let actionTitle, let action {
                Button(actionTitle) {
                    Haptics.tap()
                    action()
                }
                .buttonStyle(EmptyStateActionStyle())
                .padding(.top, 22)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(.vertical, 44)
        .padding(.horizontal, Spacing.screen)
    }
}

struct ErrorStateView: View {
    let message: String
    var retry: (() -> Void)?

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(spacing: 0) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 30, weight: .light))
                .foregroundStyle(palette.badColor)

            Rectangle()
                .fill(palette.badColor)
                .frame(width: 26, height: 1.5)
                .padding(.top, 20)

            Text("Couldn't load this")
                .font(.display(27))
                .foregroundStyle(palette.ink0Color)
                .padding(.top, 14)

            Text(message)
                .font(.ui(13.5))
                .foregroundStyle(palette.ink2Color)
                .multilineTextAlignment(.center)
                .lineSpacing(3)
                .frame(maxWidth: 320)
                .padding(.top, 8)

            if let retry {
                Button("Try again") {
                    Haptics.tap()
                    retry()
                }
                .buttonStyle(EmptyStateActionStyle())
                .padding(.top, 22)
            }
        }
        // Same reason as `EmptyStateView`: the container's ground must paint
        // the page, not a band across it.
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(.vertical, 44)
        .padding(.horizontal, Spacing.screen)
    }
}

// MARK: - Offline pill

struct OfflinePill: View {
    @Environment(\.palette) private var palette
    private var connectivity = Connectivity.shared

    var body: some View {
        Group {
            if !connectivity.isOnline || connectivity.pendingWrites > 0 {
                HStack(spacing: 6) {
                    Image(systemName: connectivity.isOnline ? "arrow.triangle.2.circlepath" : "wifi.slash")
                        .font(.system(size: 11, weight: .semibold))
                    Text(pillText)
                        .font(.ui(12, weight: .medium))
                }
                .foregroundStyle(connectivity.isOnline ? palette.warnColor : palette.ink1Color)
                .padding(.horizontal, 11)
                .padding(.vertical, 6)
                .background(Capsule().fill(.ultraThinMaterial))
                .overlay(Capsule().strokeBorder(palette.line2.color, lineWidth: 0.5))
                .transition(.move(edge: .top).combined(with: .opacity))
            }
        }
        .animation(Motion.glide, value: connectivity.isOnline)
        .animation(Motion.glide, value: connectivity.pendingWrites)
    }

    private var pillText: String {
        if !connectivity.isOnline {
            return connectivity.pendingWrites > 0
                ? "Offline · \(connectivity.pendingWrites) queued"
                : "Offline"
        }
        return "Syncing \(connectivity.pendingWrites)"
    }
}

// MARK: - Star rating

/// Five stars, in half-star steps.
///
/// Interactive ratings are set by dragging across the row — the value tracks
/// your finger continuously, so a half star is the same gesture as a whole one
/// rather than a second tap on a star you already picked. A tap is just a
/// zero-distance drag, so it lands wherever you touched: the left half of the
/// third star is 2.5.
struct StarRating: View {
    let stars: Double
    var size: CGFloat = 13
    var interactive = false
    var onChange: ((Double) -> Void)?

    @Environment(\.palette) private var palette
    /// Set only while dragging, so the row follows the finger before the value
    /// has been committed upward.
    @State private var dragValue: Double?

    private var spacing: CGFloat { size * 0.18 }
    private var shown: Double { dragValue ?? stars }

    var body: some View {
        HStack(spacing: spacing) {
            ForEach(1...5, id: \.self) { index in
                star(index)
            }
        }
        .overlay { if interactive { dragLayer } }
        .accessibilityElement()
        .accessibilityLabel("Rating")
        .accessibilityValue(shown > 0 ? "\(shown.formatted()) of 5" : "Not rated")
        .accessibilityAdjustableAction { direction in
            guard interactive else { return }
            let next = direction == .increment ? shown + 0.5 : shown - 0.5
            onChange?(min(5, max(0, next)))
        }
    }

    private func star(_ index: Int) -> some View {
        let filled = Double(index) <= shown
        let half = !filled && Double(index) - 0.5 <= shown

        return Image(systemName: filled ? "star.fill" : (half ? "star.leadinghalf.filled" : "star"))
            .font(.system(size: size))
            .foregroundStyle(filled || half ? palette.accentColor : palette.ink3Color)
            .scaleEffect(dragValue != nil && Double(index) <= shown ? 1.12 : 1)
            .animation(Motion.snap, value: shown)
    }

    private var dragLayer: some View {
        GeometryReader { geometry in
            Color.clear
                .contentShape(Rectangle())
                .gesture(
                    // `minimumDistance: 0` so a plain tap reports a location
                    // and resolves through the same mapping as a drag.
                    DragGesture(minimumDistance: 0)
                        .onChanged { drag in
                            guard !isVerticalSwipe(drag.translation) else {
                                dragValue = nil
                                return
                            }
                            let next = rating(at: drag.location.x, width: geometry.size.width)
                            guard next != dragValue else { return }
                            Haptics.select()
                            dragValue = next
                        }
                        .onEnded { drag in
                            defer { dragValue = nil }
                            guard !isVerticalSwipe(drag.translation) else { return }
                            onChange?(rating(at: drag.location.x, width: geometry.size.width))
                        }
                )
        }
    }

    /// Whether this gesture is someone scrolling the page, not rating.
    ///
    /// The star row sits inside a scroll view, and a zero-distance drag claims
    /// the touch before the scroll view can — so a swipe that merely *starts*
    /// on the stars would otherwise set a rating instead of scrolling. Ignoring
    /// vertical-dominant travel costs that one swipe its scroll, which is far
    /// cheaper than silently rating a book the reader never meant to rate.
    private func isVerticalSwipe(_ translation: CGSize) -> Bool {
        abs(translation.height) > 10 && abs(translation.height) > abs(translation.width)
    }

    /// Maps a touch x-position onto a half-star value.
    private func rating(at x: CGFloat, width: CGFloat) -> Double {
        guard width > 0 else { return 0 }
        // A dead zone at the leading edge clears the rating — otherwise the
        // lowest reachable value is 0.5 and there's no way to un-rate by drag.
        guard x > width * 0.06 else { return 0 }
        let raw = Double(x / width) * 5
        return min(5, max(0.5, (raw * 2).rounded(.up) / 2))
    }
}

// MARK: - Progress

/// A slim capsule track. `ProgressView`'s linear style renders at a fixed
/// height and can't be tinted per-item without fighting it.
struct ProgressBar: View {
    let fraction: Double
    var tint: Color
    var height: CGFloat = 3

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var filled = false

    var body: some View {
        GeometryReader { geometry in
            let clamped = min(1, max(0, fraction))
            let width = max(height, geometry.size.width * clamped)

            ZStack(alignment: .leading) {
                Capsule().fill(.white.opacity(0.14))

                Capsule()
                    // A flat bar states a number; the gradient and the lit cap
                    // make it read as distance covered.
                    .fill(
                        LinearGradient(
                            colors: [tint.opacity(0.75), tint],
                            startPoint: .leading,
                            endPoint: .trailing
                        )
                    )
                    .overlay(alignment: .trailing) {
                        Capsule()
                            .fill(.white.opacity(0.55))
                            .frame(width: height)
                            .blur(radius: 1)
                    }
                    .frame(width: filled ? width : height)
                    .shadow(color: tint.opacity(0.5), radius: 4, y: 0)
            }
            .onAppear {
                guard !filled else { return }
                guard !reduceMotion else {
                    filled = true
                    return
                }
                // Draws itself in on arrival, so the card announces where you
                // are rather than presenting a bar that was always there. Once
                // only — a host that rebuilds this view replays it, which is
                // why the Continue carousel is not inside a lazy stack.
                withAnimation(Motion.page.delay(0.12)) { filled = true }
            }
        }
        .frame(height: height)
        .accessibilityElement()
        .accessibilityLabel("Progress")
        .accessibilityValue("\(Int(min(1, max(0, fraction)) * 100)) percent")
    }
}

// MARK: - Layout helpers

/// Background that fills the whole screen including under the nav bar.
struct ScreenBackground: View {
    @Environment(\.palette) private var palette

    var body: some View {
        palette.bg0Color.ignoresSafeArea()
    }
}

extension View {
    /// Standard horizontal inset for screen content.
    func screenPadding() -> some View {
        padding(.horizontal, Spacing.screen)
    }

    /// Publishes this scroll view's offset, for chrome that reacts to scrolling.
    func trackingScrollOffset(_ offset: Binding<CGFloat>) -> some View {
        onScrollGeometryChange(for: CGFloat.self) { geometry in
            geometry.contentOffset.y + geometry.contentInsets.top
        } action: { _, value in
            offset.wrappedValue = value
        }
    }

    /// Applies the app palette + matching color scheme to a subtree.
    func themed(_ palette: Palette, scheme: ColorScheme) -> some View {
        environment(\.palette, palette)
            .preferredColorScheme(scheme)
            .tint(palette.accentColor)
    }
}

// MARK: - Formatting

enum Format {
    static func duration(_ seconds: Double) -> String {
        guard seconds.isFinite, seconds > 0 else { return "0:00" }
        let total = Int(seconds.rounded())
        let h = total / 3600
        let m = (total % 3600) / 60
        let s = total % 60
        return h > 0
            ? String(format: "%d:%02d:%02d", h, m, s)
            : String(format: "%d:%02d", m, s)
    }

    /// Wall-clock seconds it takes to play `seconds` of audio at `rate` —
    /// the rate-adjusted "time left" a 2x listener actually experiences.
    /// Non-finite or non-positive rates fall back to the unscaled value,
    /// mirroring `remaining_at_rate` in the web player.
    static func atRate(_ seconds: Double, rate: Double) -> Double {
        guard rate.isFinite, rate > 0 else { return seconds }
        return seconds / rate
    }

    /// Compact spoken form for stats and metadata rows — "4h 12m".
    static func humanDuration(_ seconds: Int64) -> String {
        guard seconds > 0 else { return "0m" }
        let h = seconds / 3600
        let m = (seconds % 3600) / 60
        if h > 0 { return m > 0 ? "\(h)h \(m)m" : "\(h)h" }
        if m > 0 { return "\(m)m" }
        return "\(seconds)s"
    }

    static func bytes(_ count: Int64) -> String {
        guard count > 0 else { return "—" }
        let formatter = ByteCountFormatter()
        formatter.countStyle = .file
        return formatter.string(fromByteCount: count)
    }

    /// Server timestamps travel as ISO-8601. A colophon row wants a date, not a
    /// wire format, so anything that parses is rendered and anything that
    /// doesn't falls through unchanged rather than showing an empty row.
    static func isoDate(_ value: String) -> String {
        let plain = ISO8601DateFormatter()
        plain.formatOptions = [.withInternetDateTime]
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]

        guard let parsed = plain.date(from: value) ?? fractional.date(from: value) else {
            return value
        }
        let out = DateFormatter()
        out.dateStyle = .medium
        out.timeStyle = .none
        return out.string(from: parsed)
    }

    /// A publication date as a book would print it.
    ///
    /// OPF metadata is inconsistent about precision — `1995`, `1995-10`, and
    /// `1995-10-01` all turn up in the same library — so each shape is rendered
    /// at the precision it actually carries rather than padded into a full date
    /// the file never claimed.
    static func looseDate(_ value: String) -> String {
        let trimmed = value.trimmingCharacters(in: .whitespaces)
        let head = String(trimmed.prefix(10))

        let patterns: [(format: String, output: String)] = [
            ("yyyy-MM-dd", "MMM d, yyyy"),
            ("yyyy-MM", "MMMM yyyy"),
            ("yyyy", "yyyy"),
        ]
        for pattern in patterns {
            let parser = DateFormatter()
            parser.locale = Locale(identifier: "en_US_POSIX")
            parser.dateFormat = pattern.format
            guard let parsed = parser.date(from: head) else { continue }
            let out = DateFormatter()
            out.dateFormat = pattern.output
            return out.string(from: parsed)
        }
        return trimmed
    }

    static func date(unix: Int64) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(unix))
        let formatter = DateFormatter()
        formatter.dateStyle = .medium
        formatter.timeStyle = .none
        return formatter.string(from: date)
    }

    /// `relativeTo` defaults to now, and exists so this and the widget's
    /// `WidgetLabels.relative` can be asserted equal on a fixed clock rather
    /// than trusted to stay in step. The widget needs its own copy because an
    /// extension cannot reach this enum — it lives in a file full of SwiftUI
    /// views — and an untestable duplicate is how the two would drift.
    static func relative(unix: Int64, relativeTo now: Date = Date()) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(unix))
        // Non-past deltas — a moments-ago write whose timestamp sits at or just
        // past `now` from clock skew — read as "just now" rather than the
        // formatter's countdown ("in 0s"). Mirrors `WidgetLabels.relative`
        // (#2358); the two are held equal by `WidgetSnapshotTests`.
        guard date < now else { return "just now" }
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(for: date, relativeTo: now)
    }
}
