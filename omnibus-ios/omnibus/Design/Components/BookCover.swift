//  BookCover.swift
//  Cover art, and the generated plate shown when a book has none.
//
//  Every cover renders into the same 2:3 box and fills it, so a grid row never
//  goes ragged when the source images disagree about aspect ratio. Books with
//  no art get a plate built from the cover-derived `accent` the indexer stores,
//  which is what stops a shelf of coverless books reading as grey noise.

import SwiftUI

/// Identity a cover needs. Taking this rather than a whole `Book` lets stats
/// rows and search hits — which carry slim payloads — use the same treatment.
struct CoverIdentity {
    var uuid: String
    var title: String
    var author: String?
    var accent: String?
    var hasCover: Bool

    init(uuid: String, title: String, author: String? = nil, accent: String? = nil, hasCover: Bool) {
        self.uuid = uuid
        self.title = title
        self.author = author
        self.accent = accent
        self.hasCover = hasCover
    }

    /// The color this book is drawn in: its extracted cover accent, else a
    /// hue derived from the title. Shared by the generated plate, the detail
    /// hero wash, and the Continue cards — resolving it separately in each let
    /// a coverless book show a purple plate under an orange wash.
    var tone: OKLCH {
        if let accent, let parsed = OKLCH.parse(cssValue: accent) {
            return parsed
        }
        var hash: UInt64 = 5381
        for byte in title.utf8 { hash = (hash &* 33) &+ UInt64(byte) }
        return OKLCH(0.55, 0.09, Double(hash % 360))
    }

    init(_ book: Book) {
        self.init(
            uuid: book.uuid,
            title: book.displayTitle,
            author: book.creators.first?.name,
            accent: book.accent,
            hasCover: book.coverURL != nil
        )
    }
}

struct BookCover: View {
    let identity: CoverIdentity
    var size: ThumbSize = .md
    var cornerRadius: CGFloat = 5
    /// When false the cover fills whatever box the parent hands it instead of
    /// imposing its own 2:3 — what a mosaic slot needs to tile without gaps.
    var keepsAspect = true

    @Environment(\.palette) private var palette

    var body: some View {
        box
            // The plate is a permanent backdrop, not the loading placeholder.
            // Some covers are transparent PNGs; with the plate only standing in
            // until the image arrives, those books render as an empty black box
            // once it loads.
            .overlay { GeneratedCoverPlate(identity: identity) }
            .overlay {
                if identity.hasCover {
                    RemoteImage(
                        path: thumbPath(size),
                        // Largest first: upscaling a bigger copy of the same
                        // cover reads better than blowing up a smaller one.
                        alternates: [ThumbSize.lg, .md, .sm]
                            .filter { $0 != size }
                            .map(thumbPath)
                    ) {
                        Color.clear
                    }
                }
            }
            .overlay { CoverLighting() }
            .clipShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
            // A hairline keeps a white-edged cover from dissolving into the
            // dark ground without reading as a drawn border.
            .overlay {
                RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                    .strokeBorder(.white.opacity(0.10), lineWidth: 0.5)
            }
    }

    private func thumbPath(_ size: ThumbSize) -> String {
        "/api/thumbs/\(identity.uuid)/\(size.rawValue)"
    }

    @ViewBuilder
    private var box: some View {
        if keepsAspect {
            Color.clear.aspectRatio(2.0 / 3.0, contentMode: .fit)
        } else {
            Color.clear
        }
    }
}

/// What turns a cover from a picture into an object: a gutter shadow down the
/// spine edge and a single soft specular across the top-left, as if the shelf
/// were lit from above. Both are weak enough not to fight artwork — the cue
/// registers as depth rather than as an effect laid over the image.
private struct CoverLighting: View {
    var body: some View {
        GeometryReader { geometry in
            let gutter = max(3, geometry.size.width * 0.07)

            ZStack {
                LinearGradient(
                    colors: [.black.opacity(0.22), .black.opacity(0.04), .clear],
                    startPoint: .leading,
                    endPoint: .trailing
                )
                .frame(width: gutter)
                .frame(maxWidth: .infinity, alignment: .leading)

                LinearGradient(
                    colors: [.white.opacity(0.09), .clear],
                    startPoint: .topLeading,
                    endPoint: .center
                )
            }
        }
        .allowsHitTesting(false)
    }
}

/// The no-art plate: an accent-derived gradient with the title set in the
/// display face, so a coverless book still reads as a distinct object.
struct GeneratedCoverPlate: View {
    let identity: CoverIdentity

    @Environment(\.palette) private var palette

    private var base: OKLCH { identity.tone }

    private var isLight: Bool { palette.bg0.l > 0.5 }

    var body: some View {
        GeometryReader { geometry in
            let unit = geometry.size.width / 100

            ZStack {
                LinearGradient(
                    colors: [
                        OKLCH(isLight ? 0.62 : 0.42, base.c * 0.85, base.h).color,
                        OKLCH(isLight ? 0.48 : 0.26, base.c * 0.65, base.h).color,
                    ],
                    startPoint: .topLeading,
                    endPoint: .bottomTrailing
                )

                // A soft highlight keeps the plate from looking like a flat
                // swatch at large sizes.
                RadialGradient(
                    colors: [.white.opacity(0.16), .clear],
                    center: UnitPoint(x: 0.3, y: 0.16),
                    startRadius: 0,
                    endRadius: geometry.size.width * 0.85
                )

                VStack(alignment: .leading, spacing: unit * 3) {
                    Rectangle()
                        .fill(.white.opacity(0.5))
                        .frame(width: unit * 22, height: max(1, unit * 0.7))

                    Text(identity.title)
                        .font(.display(unit * 11, weight: .semibold))
                        .foregroundStyle(.white.opacity(0.94))
                        .lineLimit(4)
                        .minimumScaleFactor(0.7)
                        .multilineTextAlignment(.leading)

                    if let author = identity.author {
                        Text(author.uppercased())
                            .font(.system(size: unit * 5, weight: .medium))
                            .tracking(unit * 0.35)
                            .foregroundStyle(.white.opacity(0.62))
                            .lineLimit(2)
                    }

                    Spacer(minLength: 0)
                }
                .padding(unit * 9)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            }
        }
    }
}

/// Cover with the lift that makes a grid read as objects on a shelf rather
/// than tiles in a table.
extension View {
    func coverShadow(_ strength: CGFloat = 1) -> some View {
        shadow(color: .black.opacity(0.45 * strength), radius: 10 * strength, x: 0, y: 5 * strength)
    }
}
