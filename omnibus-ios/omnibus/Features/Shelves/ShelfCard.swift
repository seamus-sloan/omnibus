//  ShelfCard.swift
//  A shelf drawn as its own contents.
//
//  A shelf's identity is the books on it, so the card leads with a mosaic of
//  its actual covers rather than a name on a coloured square. Four 2:3 covers
//  in a 2×2 grid come out 2:3 themselves, so a shelf tile drops into the same
//  grid rhythm as a book without a special case.

import SwiftUI

struct ShelfMosaic: View {
    let preview: ShelfPreview
    var cornerRadius: CGFloat = 6

    @Environment(\.palette) private var palette

    private var tone: OKLCH {
        if let accent = preview.shelf.accent, let parsed = OKLCH.parse(cssValue: accent) {
            return parsed
        }
        var hash: UInt64 = 5381
        for byte in preview.shelf.name.utf8 { hash = (hash &* 33) &+ UInt64(byte) }
        return OKLCH(0.55, 0.07, Double(hash % 360))
    }

    var body: some View {
        Color.clear
            .aspectRatio(2.0 / 3.0, contentMode: .fit)
            .overlay { content }
            .clipShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                    .strokeBorder(.white.opacity(0.10), lineWidth: 0.5)
            }
    }

    /// The arrangement follows the count rather than always laying a 2×2 grid
    /// and leaving the unfilled quadrants blank — a hole in the tile reads as a
    /// failed image load, not as a shelf that happens to hold three books.
    @ViewBuilder
    private var content: some View {
        switch preview.covers.count {
        case 0:
            emptyPlate
        case 1:
            slot(0)
        case 2:
            HStack(spacing: 1) {
                slot(0)
                slot(1)
            }
        case 3:
            // A lead cover at full height beside two stacked: a deliberate
            // arrangement, and the shelf's first book still reads largest.
            HStack(spacing: 1) {
                slot(0)
                VStack(spacing: 1) {
                    slot(1)
                    slot(2)
                }
            }
        default:
            VStack(spacing: 1) {
                HStack(spacing: 1) {
                    slot(0)
                    slot(1)
                }
                HStack(spacing: 1) {
                    slot(2)
                    slot(3)
                }
            }
        }
    }

    /// Slots fill their share of the tile rather than carrying their own 2:3,
    /// so every arrangement tiles the same box edge to edge.
    private func slot(_ index: Int) -> some View {
        BookCover(
            identity: CoverIdentity(preview.covers[index]),
            size: .sm,
            cornerRadius: 0,
            keepsAspect: false
        )
    }

    private var emptyPlate: some View {
        ZStack {
            LinearGradient(
                colors: [
                    OKLCH(0.38, tone.c * 0.7, tone.h).color,
                    OKLCH(0.22, tone.c * 0.5, tone.h).color,
                ],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )
            Image(systemName: preview.shelf.kind.glyph)
                .font(.system(size: 26, weight: .light))
                .foregroundStyle(.white.opacity(0.45))
        }
    }
}

/// Mosaic over a name and a meta line — the tile used in the shelves grid and
/// the landing rail.
struct ShelfCard: View {
    let preview: ShelfPreview
    var width: CGFloat?

    @Environment(\.palette) private var palette

    private var shelf: ShelfSummary { preview.shelf }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ShelfMosaic(preview: preview)
                .coverShadow()
                .overlay(alignment: .topTrailing) { kindBadge }

            VStack(alignment: .leading, spacing: 2) {
                Text(shelf.name)
                    .font(.ui(13, weight: .semibold))
                    .foregroundStyle(palette.ink0Color)
                    .lineLimit(1)

                Text(metaLine)
                    .font(.ui(11))
                    .foregroundStyle(palette.ink3Color)
                    .lineLimit(1)
            }
        }
        .frame(width: width)
    }

    /// Only non-default states get a badge: an unmarked tile is a plain private
    /// manual shelf, which is the common case and needs no decoration.
    @ViewBuilder
    private var kindBadge: some View {
        if shelf.kind != .manual {
            Image(systemName: shelf.kind.glyph)
                .font(.system(size: 9, weight: .bold))
                .foregroundStyle(.white)
                .frame(width: 20, height: 20)
                .background(Circle().fill(.black.opacity(0.5)))
                .background(Circle().fill(.ultraThinMaterial))
                .padding(6)
        }
    }

    private var metaLine: String {
        var parts = ["\(shelf.bookCount) book\(shelf.bookCount == 1 ? "" : "s")"]
        if shelf.kind == .smart { parts.append("Smart") }
        if shelf.visibility == .public { parts.append("Public") }
        return parts.joined(separator: " · ")
    }
}

extension ShelfKind {
    var glyph: String {
        switch self {
        case .smart: "wand.and.stars"
        case .manual: "square.stack"
        case .wishlist: "heart.fill"
        }
    }
}
