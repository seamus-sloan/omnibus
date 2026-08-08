//  UserAvatar.swift
//  A user drawn as a circle: their uploaded picture, or a monogram.
//
//  The web client has the same component; both render `/api/users/{id}/avatar`
//  and fall back to initials so a reader without a picture still reads as a
//  person rather than an empty ring.

import SwiftUI

struct UserAvatar: View {
    let id: Int64
    /// The name to draw initials from — a display name when the user set one.
    let name: String
    let hasAvatar: Bool
    var size: CGFloat = 58

    @Environment(\.palette) private var palette

    var body: some View {
        Group {
            if hasAvatar {
                RemoteImage(path: Self.path(for: id)) { monogram }
            } else {
                monogram
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .overlay(Circle().strokeBorder(.white.opacity(0.14), lineWidth: 0.5))
        // The accent glow is identity-card dressing; at list-row sizes it
        // reads as smudging, so small avatars stay flat like `AuthorAvatar`.
        .shadow(
            color: palette.accentColor.opacity(Self.shadowOpacity(for: size)),
            radius: 12,
            y: 4
        )
    }

    /// Below this size the accent shadow is suppressed (row avatars stay flat).
    static let shadowMinSize: CGFloat = 44

    /// The accent-shadow opacity for a given avatar size — full glow at or
    /// above `shadowMinSize`, none below it.
    static func shadowOpacity(for size: CGFloat) -> Double {
        size >= shadowMinSize ? 0.25 : 0
    }

    /// The cache key / request path for a user's avatar. Shared with the
    /// invalidation call after an upload so the two can't drift.
    static func path(for id: Int64) -> String { "/api/users/\(id)/avatar" }

    private var monogram: some View {
        LinearGradient(
            colors: [palette.accentColor.opacity(0.85), palette.accentSoftColor],
            startPoint: .topLeading,
            endPoint: .bottomTrailing
        )
        .overlay {
            Text(Self.initials(for: name))
                .font(.display(size * 0.45, weight: .semibold))
                .foregroundStyle(palette.accentInk.color)
        }
    }

    /// One or two uppercase initials, split on the separators a display name
    /// or a username might use. Falls back to "?" for an empty name.
    static func initials(for name: String) -> String {
        let tokens = name
            .split(whereSeparator: { $0.isWhitespace || $0 == "." || $0 == "_" || $0 == "-" })
            .filter { !$0.isEmpty }
        switch tokens.count {
        case 0: return "?"
        case 1: return tokens[0].prefix(2).uppercased()
        default: return (tokens[0].prefix(1) + tokens[1].prefix(1)).uppercased()
        }
    }
}
