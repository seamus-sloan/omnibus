//  Motion.swift
//  The app's motion vocabulary.
//
//  Every animation in Omnibus resolves to one of five named curves. Scattering
//  `.spring(response: 0.3, dampingFraction: 0.7)` literals across forty files is
//  what makes an app feel assembled rather than designed — the numbers drift,
//  and no two surfaces settle the same way. Name the curves once, and a screen
//  nobody has touched still moves like the rest of the app.

import SwiftUI

enum Motion {
    /// Picking an object up: fast out, barely any overshoot. Presses, taps,
    /// anything the finger is still touching.
    static let lift = Animation.spring(response: 0.28, dampingFraction: 0.74)

    /// Something arriving and coming to rest. Entrances, insertions, layout.
    static let settle = Animation.spring(response: 0.44, dampingFraction: 0.82)

    /// A state flip that should register instantly — selection, toggles.
    static let snap = Animation.spring(response: 0.22, dampingFraction: 0.8)

    /// Long travel across the screen. Sheets, bars, docking players.
    static let glide = Animation.spring(response: 0.5, dampingFraction: 0.88)

    /// Paper, not rubber: an eased curve with no bounce at all, for anything
    /// that should feel like a page rather than a control.
    static let page = Animation.timingCurve(0.2, 0.6, 0.3, 1, duration: 0.4)
}

// MARK: - Entrance cascade

/// Fades and lifts a view into place, staggered by its position in a list.
///
/// The stagger is deliberately short and capped: the point is that the shelf
/// *fills*, not that the user waits for it. Delay is taken modulo a small
/// window so a cell scrolled to on page nine animates like one on page one
/// instead of arriving half a second late.
private struct CascadeIn: ViewModifier {
    let index: Int

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var shown = false

    private var delay: Double {
        min(Double(index % 8) * 0.04, 0.32)
    }

    func body(content: Content) -> some View {
        content
            .opacity(shown ? 1 : 0)
            .offset(y: shown ? 0 : 12)
            .scaleEffect(shown ? 1 : 0.985, anchor: .bottom)
            .onAppear {
                guard !shown else { return }
                guard !reduceMotion else {
                    shown = true
                    return
                }
                withAnimation(Motion.settle.delay(delay)) { shown = true }
            }
    }
}

extension View {
    /// Staggered entrance for items in a grid or rail.
    func cascadeIn(index: Int) -> some View {
        modifier(CascadeIn(index: index))
    }
}

// MARK: - Physical press

/// Press feedback for anything standing in for a physical book.
///
/// A book tips toward you when you pick it up, so the press rotates about the
/// spine rather than only scaling. The angle is small enough to read as weight
/// instead of as a card flip.
struct BookPressStyle: ButtonStyle {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        let pressed = configuration.isPressed && !reduceMotion

        return configuration.label
            .rotation3DEffect(
                .degrees(pressed ? 4.5 : 0),
                axis: (x: 0, y: 1, z: 0),
                anchor: .leading,
                perspective: 0.4
            )
            .scaleEffect(pressed ? 0.975 : 1)
            .animation(Motion.lift, value: configuration.isPressed)
    }
}

// MARK: - Ambient wash

/// The slow colour drift behind the top of the library.
///
/// Omnibus already knows the dominant colour of every cover; this is what makes
/// the app *wear* it. Two out-of-focus blooms in the current book's tone drift
/// against each other on a long cycle, so the home screen is quietly tinted by
/// whatever you're reading and changes when that changes.
struct AmbientWash: View {
    let tone: OKLCH

    @Environment(\.palette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var drift = false

    private var isLight: Bool { palette.bg0.l > 0.5 }

    var body: some View {
        // `Color.clear` is what makes this layer flexible. Left as a bare
        // `ZStack`, the layer takes the largest circle's intrinsic width and the
        // mask below clips the blur to that box — a visible hard-edged
        // rectangle rather than a bloom.
        Color.clear
            .overlay {
                ZStack {
                    bloom(
                        color: OKLCH(isLight ? 0.82 : 0.46, max(0.05, tone.c) * 1.1, tone.h).color,
                        size: 340,
                        offset: CGSize(width: drift ? -110 : -50, height: drift ? -120 : -170)
                    )
                    // The second bloom is always the app accent, never a
                    // rotation of the book's hue: a navy or grey cover would
                    // otherwise wash the home screen in cold smoke. Warmth is
                    // the constant, the book supplies the variable.
                    bloom(
                        color: OKLCH(isLight ? 0.86 : 0.42, 0.09, palette.accent.h).color,
                        size: 260,
                        offset: CGSize(width: drift ? 130 : 70, height: drift ? -200 : -120)
                    )
                }
            }
            .opacity(isLight ? 0.45 : 0.6)
            // Fades out before the grid so content never sits on top of colour.
            .mask(
                LinearGradient(
                    stops: [
                        .init(color: .black, location: 0),
                        .init(color: .black.opacity(0.55), location: 0.4),
                        .init(color: .clear, location: 1),
                    ],
                    startPoint: .top,
                    endPoint: .bottom
                )
            )
            .allowsHitTesting(false)
            .onAppear {
                guard !reduceMotion else { return }
                withAnimation(.easeInOut(duration: 16).repeatForever(autoreverses: true)) {
                    drift = true
                }
            }
    }

    private func bloom(color: Color, size: CGFloat, offset: CGSize) -> some View {
        Circle()
            .fill(color)
            .frame(width: size, height: size)
            .blur(radius: 80)
            .offset(offset)
    }
}

// MARK: - Cover → detail zoom

/// Namespace for the cover-to-detail zoom, published by the surface that owns
/// the source cover.
///
/// Opt-in rather than app-wide: a zoom transition needs exactly one live source
/// matching the destination, and the same book can be on screen in several
/// stacks at once. A stack that publishes nothing gets the stock push.
private struct BookZoomNamespaceKey: EnvironmentKey {
    static let defaultValue: Namespace.ID? = nil
}

extension EnvironmentValues {
    var bookZoomNamespace: Namespace.ID? {
        get { self[BookZoomNamespaceKey.self] }
        set { self[BookZoomNamespaceKey.self] = newValue }
    }
}

extension View {
    /// Marks this cover as the origin of the zoom into `uuid`'s detail screen.
    @ViewBuilder
    func bookZoomSource(_ uuid: String, in namespace: Namespace.ID?) -> some View {
        if let namespace {
            matchedTransitionSource(id: uuid, in: namespace)
        } else {
            self
        }
    }

    /// Applies the zoom on the detail screen, when a source published one.
    @ViewBuilder
    func bookZoomDestination(_ uuid: String, in namespace: Namespace.ID?) -> some View {
        if let namespace {
            navigationTransition(.zoom(sourceID: uuid, in: namespace))
        } else {
            self
        }
    }
}
