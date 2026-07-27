//  BrandMark.swift
//  The Omnibus logo, shared with the web build.
//
//  `BrandMark` in the asset catalogue is a copy of the web's
//  `frontend/assets/omnibus-stoat.png` — the same file it serves as its top-nav
//  mark, its auth-shell mark, and its favicon. Keep the two in sync when either
//  moves.

import SwiftUI

/// The stoat over an open book. Sized by height; the art is square, so the
/// frame is too.
///
/// Deliberately untinted: the web renders the orange mark as-is under all four
/// themes, monochrome one included, and a client that recoloured it per palette
/// would stop being the same logo.
struct OmnibusMark: View {
    /// Matches the web's 26px mark against its 22px wordmark when paired with
    /// the 14pt masthead wordmark.
    var height: CGFloat = 16

    var body: some View {
        Image("BrandMark")
            .resizable()
            .scaledToFit()
            .frame(width: height, height: height)
            .accessibilityHidden(true)
    }
}
