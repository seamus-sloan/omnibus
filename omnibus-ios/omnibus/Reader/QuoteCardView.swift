//  QuoteCardView.swift
//  Turning a passage into something worth posting.
//
//  The card is a real SwiftUI view rendered twice: once at the width the sheet
//  has, and once at 1080pt for export. Every metric is a fraction of that
//  width, so the PNG the reader shares is exactly the card they were looking
//  at — a canvas renderer with its own font stack, which is how the web build
//  does it, can only ever approximate the preview.

import SwiftUI

extension String {
    /// One space per run of whitespace, ends trimmed.
    ///
    /// A passage carries the EPUB source file's own line breaks and
    /// indentation. They are invisible on the page, where the browser
    /// collapses them, and ragged everywhere else the text is re-set — a
    /// note, a paste, a quote card. Newer highlights are collapsed in the
    /// glue; rows the web client wrote, or this app wrote before that, are
    /// not, so anything re-setting a stored passage collapses it again here.
    var collapsingWhitespace: String {
        replacingOccurrences(of: "\\s+", with: " ", options: .regularExpression)
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

/// A card background: a ground tone plus whether its ink is light or dark.
struct QuoteStyle: Identifiable {
    var id: String
    var base: OKLCH
    var lightInk: Bool

    /// A two-stop wash rather than a flat fill — a flat colour behind serif
    /// type reads as a placeholder, and the drift is what gives the card
    /// depth at thumbnail size.
    var gradient: LinearGradient {
        LinearGradient(
            colors: [
                OKLCH(base.l, base.c, base.h).color,
                OKLCH(
                    max(0.06, base.l - (lightInk ? 0.10 : -0.05)),
                    base.c * 0.85,
                    base.h + 12
                ).color,
            ],
            startPoint: .topLeading,
            endPoint: .bottomTrailing
        )
    }

    var ink: Color {
        lightInk ? OKLCH(0.95, 0.012, base.h).color : OKLCH(0.24, 0.02, base.h).color
    }

    static func book(_ tone: OKLCH) -> QuoteStyle {
        QuoteStyle(id: "book", base: OKLCH(min(0.46, max(0.28, tone.l)), max(0.05, tone.c), tone.h), lightInk: true)
    }

    static let presets: [QuoteStyle] = [
        QuoteStyle(id: "ink", base: OKLCH(0.19, 0.012, 70), lightInk: true),
        QuoteStyle(id: "paper", base: OKLCH(0.93, 0.017, 80), lightInk: false),
        QuoteStyle(id: "moss", base: OKLCH(0.38, 0.075, 155), lightInk: true),
        QuoteStyle(id: "rose", base: OKLCH(0.44, 0.11, 15), lightInk: true),
        QuoteStyle(id: "dusk", base: OKLCH(0.36, 0.085, 285), lightInk: true),
        QuoteStyle(id: "sea", base: OKLCH(0.40, 0.075, 230), lightInk: true),
    ]
}

/// Card shapes, named for what they're for.
enum QuoteRatio: String, CaseIterable, Identifiable {
    case square, portrait, story

    var id: String { rawValue }

    var value: CGFloat {
        switch self {
        case .square: 1
        case .portrait: 4.0 / 5.0
        case .story: 9.0 / 16.0
        }
    }

    var label: String {
        switch self {
        case .square: "Square"
        case .portrait: "Post"
        case .story: "Story"
        }
    }
}

// MARK: - The card

/// The card itself. Draws identically at any width — every dimension below is
/// a fraction of `width`.
struct QuoteCard: View {
    let quote: String
    let title: String
    let author: String?
    let style: QuoteStyle
    let ratio: QuoteRatio
    let width: CGFloat

    private var height: CGFloat { width / ratio.value }
    private var pad: CGFloat { width * 0.094 }

    /// Long passages step down rather than shrink to fit: a card whose type
    /// scales continuously with length has no typographic identity, and the
    /// steps keep every card recognisably the same object.
    private var quoteSize: CGFloat {
        let count = quote.count
        let scale: CGFloat
        switch count {
        case ..<90: scale = 0.082
        case ..<180: scale = 0.066
        case ..<320: scale = 0.052
        case ..<520: scale = 0.042
        default: scale = 0.034
        }
        return width * scale
    }

    private var text: String { quote.collapsingWhitespace }

    var body: some View {
        ZStack(alignment: .topLeading) {
            style.gradient

            VStack(alignment: .leading, spacing: 0) {
                Spacer(minLength: 0)

                Text(text)
                    .font(.system(size: quoteSize, weight: .regular, design: .serif))
                    .foregroundStyle(style.ink)
                    .lineSpacing(quoteSize * 0.34)
                    .multilineTextAlignment(.leading)
                    .fixedSize(horizontal: false, vertical: true)
                    .minimumScaleFactor(0.55)
                    // An oversized opening quote, tucked behind the first
                    // line. Anchored to the text rather than to the card, so
                    // it stays with the passage on a tall story card instead
                    // of stranding itself at the top edge.
                    .background(alignment: .topLeading) {
                        Text("\u{201C}")
                            .font(.system(size: width * 0.30, design: .serif))
                            .foregroundStyle(style.ink.opacity(0.13))
                            .fixedSize()
                            .offset(x: -width * 0.035, y: -width * 0.135)
                    }

                Spacer(minLength: 0)

                Rectangle()
                    .fill(style.ink.opacity(0.22))
                    .frame(width: width * 0.14, height: max(1, width * 0.004))
                    .padding(.bottom, width * 0.042)

                if let author = author?.nilIfBlank {
                    Text(author.uppercased())
                        .font(.system(size: width * 0.026, weight: .semibold))
                        .tracking(width * 0.0036)
                        .foregroundStyle(style.ink.opacity(0.75))
                        .padding(.bottom, width * 0.012)
                }
                Text(title)
                    .font(.system(size: width * 0.033, design: .serif))
                    .italic()
                    .foregroundStyle(style.ink.opacity(0.62))
                    .lineLimit(2)
            }
            .padding(pad)

            // The mark, quiet enough to be a signature rather than a watermark.
            Text("OMNIBUS")
                .font(.system(size: width * 0.021, weight: .semibold))
                .tracking(width * 0.006)
                .foregroundStyle(style.ink.opacity(0.34))
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomTrailing)
                .padding(pad)
        }
        .frame(width: width, height: height)
        .clipped()
    }
}

// MARK: - The composer

/// Picking how a passage should look, and sending it somewhere.
struct QuoteCardSheet: View {
    let quote: String
    let book: Book

    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    @State private var styleID: String = "book"
    @State private var ratio: QuoteRatio = .square
    @State private var rendering = false

    /// The book's own colour first — a card in the tone of its cover is the
    /// one that reads as *this* book rather than as a template.
    private var styles: [QuoteStyle] {
        [QuoteStyle.book(CoverIdentity(book).tone)] + QuoteStyle.presets
    }

    private var style: QuoteStyle {
        styles.first { $0.id == styleID } ?? styles[0]
    }

    private var author: String? { book.creators.first?.name }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 26) {
                    preview
                    controls
                    actions
                }
                .screenPadding()
                .padding(.top, Spacing.md)
                .padding(.bottom, 36)
            }
            .scrollIndicators(.hidden)
            .background(ScreenBackground())
            .navigationTitle("Quote card")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .tint(palette.accentColor)
    }

    private var preview: some View {
        GeometryReader { geometry in
            QuoteCard(
                quote: quote,
                title: book.displayTitle,
                author: author,
                style: style,
                ratio: ratio,
                width: geometry.size.width
            )
            .clipShape(RoundedRectangle(cornerRadius: Radius.lg, style: .continuous))
            .shadow(color: .black.opacity(0.28), radius: 22, y: 10)
        }
        // The card sizes itself off the width it is given, so the container
        // has to reserve the height that width will produce.
        .aspectRatio(ratio.value, contentMode: .fit)
        .animation(Motion.settle, value: ratio)
        .animation(Motion.settle, value: styleID)
    }

    private var controls: some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            SectionLabel("Background")
            HStack(spacing: 12) {
                ForEach(styles) { option in
                    Button {
                        guard styleID != option.id else { return }
                        Haptics.select()
                        withAnimation(Motion.snap) { styleID = option.id }
                    } label: {
                        Circle()
                            .fill(option.gradient)
                            .frame(width: 34, height: 34)
                            .overlay(
                                Circle().strokeBorder(.white.opacity(0.18), lineWidth: 1)
                            )
                            .overlay {
                                if styleID == option.id {
                                    Circle()
                                        .strokeBorder(palette.accentColor, lineWidth: 2)
                                        .padding(-4)
                                }
                            }
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(option.id.capitalized)
                    .accessibilityAddTraits(styleID == option.id ? [.isSelected, .isButton] : .isButton)
                }
                Spacer(minLength: 0)
            }

            SectionLabel("Shape")
            PillSelector(
                options: QuoteRatio.allCases,
                label: \.label,
                selection: $ratio
            )
        }
    }

    private var actions: some View {
        VStack(spacing: Spacing.md) {
            Button {
                Haptics.tap()
                share()
            } label: {
                Label("Share", systemImage: "square.and.arrow.up")
            }
            .buttonStyle(FilledButtonStyle())
            .disabled(rendering)

            Button {
                Haptics.success()
                if let image = render() {
                    UIPasteboard.general.image = image
                }
            } label: {
                Label("Copy image", systemImage: "doc.on.doc")
            }
            .buttonStyle(FilledButtonStyle(prominent: false))
            .disabled(rendering)
        }
    }

    private func share() {
        rendering = true
        defer { rendering = false }
        guard let image = render() else { return }
        ShareSheet.present(items: [image])
    }

    /// The card at export size. 1080pt wide with a scale of 1 gives a 1080px
    /// PNG — the size every social surface wants — and because the card's
    /// metrics are all fractions of its width, it is pixel-for-pixel the
    /// preview, just larger.
    @MainActor
    private func render() -> UIImage? {
        let renderer = ImageRenderer(
            content: QuoteCard(
                quote: quote,
                title: book.displayTitle,
                author: author,
                style: style,
                ratio: ratio,
                width: 1080
            )
        )
        renderer.scale = 1
        renderer.isOpaque = true
        return renderer.uiImage
    }
}
