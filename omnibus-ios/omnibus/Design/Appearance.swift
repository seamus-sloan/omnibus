//  Appearance.swift
//  UIKit-level defaults SwiftUI has no modifier for.
//
//  `Font.display` puts the editorial face on everything the app draws itself,
//  but navigation titles are rendered by UIKit and stay on the system sans —
//  which is why a pushed "Shelves" or "Authors" screen used to read as a
//  different app from the library that launched it. Setting it once here is
//  what carries the identity into every stack without touching 25 call sites.

import SwiftUI
import UIKit

enum Appearance {
    static func apply() {
        let titles: [NSAttributedString.Key: Any] = [.font: serif(size: 17, weight: .medium)]
        let largeTitles: [NSAttributedString.Key: Any] = [.font: serif(size: 34, weight: .regular)]

        // Transparent in every state, matching the flat chrome the rest of the
        // app uses — the tab bar is custom and every screen carries its own
        // ground. Separation between a title and the content scrolling under it
        // comes from iOS 26's scroll edge effect, not from a bar fill.
        let bar = UINavigationBarAppearance()
        bar.configureWithTransparentBackground()
        bar.titleTextAttributes = titles
        bar.largeTitleTextAttributes = largeTitles

        UINavigationBar.appearance().standardAppearance = bar
        UINavigationBar.appearance().compactAppearance = bar
        UINavigationBar.appearance().scrollEdgeAppearance = bar
    }

    /// New York — the system's editorial face, and the same register as the
    /// web build's Instrument Serif without bundling a webfont.
    private static func serif(size: CGFloat, weight: UIFont.Weight) -> UIFont {
        let base = UIFont.systemFont(ofSize: size, weight: weight)
        guard let descriptor = base.fontDescriptor.withDesign(.serif) else { return base }
        return UIFont(descriptor: descriptor, size: size)
    }
}
