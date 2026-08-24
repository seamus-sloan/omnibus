//  MetadataFetchCards.swift
//  The fetch-metadata sheet's visual vocabulary: the source badge, one
//  candidate row, one comparison card, the cover card, the per-source strip,
//  and the placeholders that stand in while a request is out.
//
//  Kept apart from `MetadataFetchSheet` so the sheet reads as its three phases
//  and nothing else. Every card here is a leaf — it takes strings and closures,
//  never the network — which is what lets the sheet own all the async work in
//  one place.

import SwiftUI

// MARK: - Source badge

/// Which provider offered a thing. Tinted per source, so two candidates from
/// two catalogs are told apart at a glance rather than by reading the label.
struct ProviderBadge: View {
    let provider: MetadataProvider
    var compact = false

    @Environment(\.palette) private var palette

    private var tint: Color {
        OKLCH(palette.bg0.l > 0.5 ? 0.46 : 0.80, 0.10, provider.badgeHue).color
    }

    var body: some View {
        Text(provider.displayName.uppercased())
            .font(.monoUI(compact ? 8.5 : 9, weight: .semibold))
            .tracking(0.6)
            .foregroundStyle(tint)
            .padding(.horizontal, 6)
            .padding(.vertical, 2.5)
            .background(Capsule().fill(tint.opacity(0.13)))
            .overlay(Capsule().strokeBorder(tint.opacity(0.32), lineWidth: 0.5))
    }
}

// MARK: - Candidate row

/// One candidate. Selecting it is the row's only action, so the whole row is
/// the control.
struct EditionCandidateCard: View {
    let edition: ProviderEdition
    /// How many fields this candidate would change, against the draft as it
    /// stands — the one thing a list of near-identical printings can't tell you
    /// by looking, and the reason this row is worth more than the web's.
    let changes: Int
    let action: () -> Void

    @Environment(\.palette) private var palette

    var body: some View {
        Button(action: action) {
            HStack(alignment: .top, spacing: Spacing.md) {
                cover.frame(width: 46)

                VStack(alignment: .leading, spacing: 3) {
                    Text(edition.title)
                        .font(.display(18))
                        .foregroundStyle(palette.ink0Color)
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)

                    Text(MetadataFetchFlow.authorsLine(edition))
                        .font(.ui(12.5))
                        .foregroundStyle(palette.ink2Color)
                        .lineLimit(1)

                    Text(MetadataFetchFlow.imprintLine(edition))
                        .font(.monoUI(10))
                        .foregroundStyle(palette.ink3Color)
                        .lineLimit(1)

                    HStack(spacing: 6) {
                        ProviderBadge(provider: edition.source)
                        changesPill
                    }
                    .padding(.top, 3)
                }

                Spacer(minLength: 0)

                Image(systemName: "chevron.right")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(palette.ink3Color)
                    .padding(.top, 6)
            }
            .padding(Spacing.md)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                    .fill(palette.bg1Color)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                    .strokeBorder(palette.line2Color, lineWidth: 0.5)
            )
        }
        .buttonStyle(PressableStyle())
        // Explicit rather than inherited from the row's text: the accessible
        // name is otherwise the whole card, which reads as a paragraph and
        // makes two printings of one book indistinguishable by name.
        .accessibilityLabel("Compare \(edition.title) from \(edition.source.displayName)")
        .accessibilityHint(changes > 0 ? "\(changes) fields differ" : "Matches what you have")
    }

    @ViewBuilder
    private var cover: some View {
        if let url = MetadataFetchFlow.coverURL(edition) {
            ExternalImage(url: url) { CoverPlate(title: edition.title) }
                .aspectRatio(2.0 / 3.0, contentMode: .fit)
                .clipShape(RoundedRectangle(cornerRadius: Radius.sm, style: .continuous))
        } else {
            CoverPlate(title: edition.title)
        }
    }

    @ViewBuilder
    private var changesPill: some View {
        if changes > 0 {
            Text("\(changes) differ")
                .font(.monoUI(9, weight: .semibold))
                .tracking(0.5)
                .foregroundStyle(palette.accentColor)
                .padding(.horizontal, 6)
                .padding(.vertical, 2.5)
                .background(Capsule().fill(palette.accentColor.opacity(0.13)))
        } else {
            Text("MATCHES YOURS")
                .font(.monoUI(9, weight: .medium))
                .tracking(0.5)
                .foregroundStyle(palette.ink3Color)
        }
    }
}

/// The lettered stand-in for a candidate whose provider offered no art.
struct CoverPlate: View {
    let title: String
    var corner: CGFloat = Radius.sm

    @Environment(\.palette) private var palette

    var body: some View {
        RoundedRectangle(cornerRadius: corner, style: .continuous)
            .fill(palette.coverFallbackBg.color)
            .aspectRatio(2.0 / 3.0, contentMode: .fit)
            .overlay {
                Text(title.prefix(1).uppercased())
                    .font(.display(24))
                    .foregroundStyle(palette.coverFallbackInk.color.opacity(0.5))
            }
    }
}

// MARK: - Comparison card

/// One field, stacked rather than columned.
///
/// The web picker lays a field out as two side-by-side columns with an arrow
/// between them, which needs a desktop's width to read. On a phone the same
/// information is labelled blocks down the card, so nothing is truncated and
/// the attribution stays attached to the value it belongs to.
///
/// A card carrying an unsaved change grows a third block — what the book had —
/// but **keeps showing the source's value and its Take control**.
/// `MetadataFetchField.isStaged` is baseline-relative by design, so it is true
/// for any unsaved edit: one typed into the form before this sheet opened, or
/// one taken from a different candidate. Reading it as "this card is finished"
/// hid the very value the reader opened this candidate to see — while the bar's
/// "Take all", which is driven by `differs`, still counted and wrote it.
struct EditionFieldCard: View {
    let field: MetadataFetchField
    let sourceName: String
    /// What the draft holds right now.
    let current: String
    /// What the provider offers, or "" when it offers nothing.
    let offered: String
    /// What the book held when the editor loaded it.
    let original: String
    let isStaged: Bool
    /// True while the detail re-fetch is in flight — this record is still the
    /// thin search hit, so nothing on it may be taken yet.
    let isBusy: Bool
    let onTake: () -> Void
    let onUndo: () -> Void

    @Environment(\.palette) private var palette
    @State private var expanded = false

    /// Whether the source's value is worth offering: it has one, the record has
    /// settled, and it isn't already what the draft holds.
    private var canTake: Bool { !offered.isEmpty && !isBusy && offered != current }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header
            VStack(alignment: .leading, spacing: 9) {
                value(marker: isStaged ? "Now" : "Yours", text: current, emphasis: isStaged)
                // Only once it differs from what the book had — otherwise this
                // is the same string twice under two markers.
                if isStaged, original != current {
                    value(marker: "Was", text: original, emphasis: false)
                }
                // Always rendered. A staged field's card used to drop this
                // block, which left the candidate's value unreadable and
                // untakeable on exactly the row a reader had come to compare.
                if offered != current {
                    value(marker: sourceName, text: offered, emphasis: !isStaged)
                }
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                .fill(palette.bg1Color)
        )
        .overlay(
            RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                .strokeBorder(
                    isStaged ? palette.accentColor.opacity(0.5) : palette.line2Color,
                    lineWidth: isStaged ? 1 : 0.5
                )
        )
        .animation(Motion.snap, value: isStaged)
    }

    private var header: some View {
        HStack(spacing: 6) {
            Text(field.label.uppercased())
                .font(.monoUI(10, weight: .medium))
                .tracking(0.7)
                .foregroundStyle(isStaged ? palette.accentColor : palette.ink3Color)

            if isStaged {
                Circle()
                    .fill(palette.accentColor)
                    .frame(width: 4, height: 4)
                    .transition(.scale.combined(with: .opacity))
            }

            Spacer(minLength: Spacing.sm)

            // Two independent controls, not one that changes meaning: a field
            // can simultaneously carry an unsaved edit *and* have a value on
            // offer from this candidate, and both actions have to stay
            // reachable.
            HStack(spacing: 6) {
                if isStaged { undoButton }
                if canTake || !isStaged { takeButton }
            }
        }
    }

    private var takeButton: some View {
        Button {
            Haptics.select()
            onTake()
        } label: {
            HStack(spacing: 4) {
                Image(systemName: "arrow.down")
                    .font(.system(size: 10, weight: .bold))
                Text("Take")
            }
            .font(.ui(12.5, weight: .semibold))
            .foregroundStyle(palette.accentInkColor)
            .padding(.horizontal, 11)
            .padding(.vertical, 6)
            .background(Capsule().fill(palette.accentColor))
            .opacity(canTake ? 1 : 0.4)
        }
        .buttonStyle(.plain)
        // A provider not knowing a field must never blank out a value you
        // have — and neither must a card about to be replaced.
        .disabled(!canTake)
        .accessibilityLabel("Take \(field.label) from \(sourceName)")
    }

    /// Back to what the book had. Present only while the field carries an
    /// unsaved change, whoever made it.
    private var undoButton: some View {
        Button {
            Haptics.select()
            onUndo()
        } label: {
            HStack(spacing: 4) {
                Image(systemName: "arrow.uturn.backward")
                    .font(.system(size: 10, weight: .bold))
                Text("Undo")
            }
            .font(.ui(12.5, weight: .semibold))
            .foregroundStyle(palette.ink1Color)
            .padding(.horizontal, 11)
            .padding(.vertical, 6)
            .background(Capsule().fill(palette.bg2Color))
        }
        .buttonStyle(.plain)
        .disabled(isBusy)
        .accessibilityLabel("Undo the change to \(field.label)")
    }

    private func value(marker: String, text: String, emphasis: Bool) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(marker.uppercased())
                .font(.monoUI(9, weight: .medium))
                .tracking(0.6)
                .foregroundStyle(palette.ink3Color.opacity(0.85))

            Text(text.isEmpty ? MetadataFetchFlow.empty : text)
                .font(field.isProse ? .display(15.5) : .ui(14.5, weight: emphasis ? .medium : .regular))
                .foregroundStyle(valueInk(text: text, emphasis: emphasis))
                .lineLimit(expanded ? nil : (field.isProse ? 3 : 3))
                .multilineTextAlignment(.leading)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)

            if field.isProse, text.count > 160 {
                Button(expanded ? "Show less" : "Show more") {
                    withAnimation(Motion.settle) { expanded.toggle() }
                }
                .font(.ui(12, weight: .medium))
                .foregroundStyle(palette.accentColor)
                .buttonStyle(.plain)
            }
        }
    }

    private func valueInk(text: String, emphasis: Bool) -> Color {
        if text.isEmpty { return palette.ink3Color }
        return emphasis ? palette.ink0Color : palette.ink2Color
    }
}

// MARK: - Cover card

/// The cover, which is a field on this screen like any other and the only one
/// that cannot stage: applying it means the *server* fetching the provider's
/// image on the reader's behalf, so this is the one card that writes
/// immediately — and it has to say so rather than implying it will be saved
/// with the rest.
struct EditionCoverCard: View {
    let identity: CoverIdentity
    let sourceName: String
    let sourceURL: String?
    let isBusy: Bool
    /// Bumped after a successful apply so the current thumbnail is re-fetched
    /// rather than served from the image cache it was already in.
    let revision: Int
    let status: String?
    let isApplying: Bool
    let onApply: () -> Void

    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 11) {
            Text("COVER")
                .font(.monoUI(10, weight: .medium))
                .tracking(0.7)
                .foregroundStyle(palette.ink3Color)

            HStack(alignment: .center, spacing: Spacing.md) {
                labelled("Yours") {
                    BookCover(identity: identity, size: .sm, cornerRadius: Radius.sm)
                        .id(revision)
                }

                Image(systemName: "arrow.right")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(palette.ink3Color)
                    .padding(.top, 14)

                labelled(sourceName) {
                    if let sourceURL {
                        ExternalImage(url: sourceURL) { CoverPlate(title: "?") }
                            .aspectRatio(2.0 / 3.0, contentMode: .fit)
                            .clipShape(
                                RoundedRectangle(cornerRadius: Radius.sm, style: .continuous)
                            )
                    } else {
                        CoverPlate(title: "?")
                    }
                }

                Spacer(minLength: 0)
            }

            Button {
                Haptics.tap()
                onApply()
            } label: {
                HStack(spacing: 6) {
                    if isApplying {
                        ProgressView().controlSize(.small)
                    } else {
                        Image(systemName: "photo.on.rectangle.angled")
                            .font(.system(size: 12, weight: .semibold))
                    }
                    Text("Use this cover")
                }
                .font(.ui(13.5, weight: .semibold))
                .foregroundStyle(palette.accentInkColor)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 10)
                .background(
                    RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                        .fill(palette.accentColor)
                )
                .opacity(sourceURL == nil || isBusy || isApplying ? 0.45 : 1)
            }
            .buttonStyle(.plain)
            .disabled(sourceURL == nil || isBusy || isApplying)

            // The wording is the contract: this is the one card that doesn't
            // wait for Save.
            Text(status ?? "Applies immediately \u{b7} not staged with the fields")
                .font(.monoUI(10))
                .foregroundStyle(status == nil ? palette.ink3Color : palette.accentColor)
                .accessibilityAddTraits(.updatesFrequently)
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                .fill(palette.bg1Color)
        )
        .overlay(
            RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                .strokeBorder(palette.line2Color, lineWidth: 0.5)
        )
    }

    private func labelled<Content: View>(
        _ marker: String, @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(marker.uppercased())
                .font(.monoUI(9, weight: .medium))
                .tracking(0.6)
                .foregroundStyle(palette.ink3Color.opacity(0.85))
                .lineLimit(1)
            content().frame(width: 62)
        }
    }
}

// MARK: - Per-source strip

/// What each source contributed.
///
/// It exists because a short list has several causes and they are not
/// interchangeable: a provider that answered with nothing, one this instance
/// has no key for, and one that could not be reached all look identical from
/// the list alone. Dropping a failed provider silently is the failure this
/// strip prevents.
struct ProviderSourceStrip: View {
    let sources: [ProviderSearchSource]

    @Environment(\.palette) private var palette

    /// A phone has no hover, so a failure's own message can't ride along as a
    /// tooltip the way it does on the web — it is printed under the strip
    /// instead, where it is readable without becoming an error log.
    private var failures: [String] {
        sources.compactMap { source in
            guard case let .failed(message) = source.status, let text = message.nilIfBlank else {
                return nil
            }
            return "\(source.displayName): \(text)"
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            Text("SOURCES")
                .font(.monoUI(9, weight: .medium))
                .tracking(0.7)
                .foregroundStyle(palette.ink3Color)

            FlowLayout(spacing: 6, lineSpacing: 6) {
                ForEach(sources) { source in
                    row(source)
                }
            }

            ForEach(failures, id: \.self) { line in
                Text(line)
                    .font(.ui(11.5))
                    .foregroundStyle(palette.ink3Color)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .contain)
    }

    private func row(_ source: ProviderSearchSource) -> some View {
        let status = MetadataFetchFlow.sourceStatus(source.status)
        return HStack(spacing: 5) {
            Text(source.displayName)
                .font(.ui(12, weight: .medium))
                .foregroundStyle(palette.ink2Color)
            Text(status.text)
                .font(.monoUI(11, weight: .medium))
                .foregroundStyle(status.isProblem ? palette.warnColor : palette.ink3Color)
        }
        .padding(.horizontal, 9)
        .padding(.vertical, 5)
        .background(Capsule().fill(palette.bg1Color))
        .overlay(Capsule().strokeBorder(palette.line2Color, lineWidth: 0.5))
        .accessibilityElement()
        .accessibilityLabel("\(source.displayName), \(status.text)")
    }
}

// MARK: - Placeholders

/// What the sheet shows while a request is out: the shape of the answer,
/// without any of its content.
///
/// Deliberately not a spinner. The reader is about to read a list of cards, and
/// a placeholder in that list's shape means the real rows land where the eye is
/// already looking instead of shifting it.
struct MetadataFetchSkeleton: View {
    var rows = 4
    /// Candidate rows lead with a cover; comparison cards don't.
    var showsCover = true

    @Environment(\.palette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var dim = false

    var body: some View {
        VStack(spacing: Spacing.sm) {
            ForEach(0..<rows, id: \.self) { _ in
                HStack(alignment: .top, spacing: Spacing.md) {
                    if showsCover {
                        bar(width: 46, height: 69, corner: Radius.sm)
                    }
                    VStack(alignment: .leading, spacing: 7) {
                        bar(width: 190, height: 13)
                        bar(width: 120, height: 10)
                        bar(width: 150, height: 10)
                    }
                    Spacer(minLength: 0)
                }
                .padding(Spacing.md)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(
                    RoundedRectangle(cornerRadius: Radius.lg, style: .continuous)
                        .fill(palette.bg1Color)
                )
            }
        }
        .opacity(dim ? 0.55 : 1)
        .onAppear {
            guard !reduceMotion else { return }
            withAnimation(.easeInOut(duration: 0.9).repeatForever(autoreverses: true)) {
                dim = true
            }
        }
        .accessibilityHidden(true)
    }

    private func bar(width: CGFloat, height: CGFloat, corner: CGFloat = 3) -> some View {
        RoundedRectangle(cornerRadius: corner, style: .continuous)
            .fill(palette.bg2Color)
            .frame(width: width, height: height)
    }
}
