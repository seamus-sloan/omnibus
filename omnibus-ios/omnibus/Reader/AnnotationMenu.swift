//  AnnotationMenu.swift
//  The passage menu and the note composer.
//
//  One menu serves both directions: a fresh selection (no colour yet) and a
//  tap on a highlight already on the page. It replaces the system callout
//  rather than joining it — WebKit's selection is off inside the section, so
//  there is no second menu to compete with, and the verbs a reader wants on a
//  passage (a colour, a note, a quote card) are ones no system menu offers.
//  Look Up and Translate are carried over so nothing the callout gave up is
//  actually lost.

import SwiftUI

/// What the menu can be asked to do with a passage.
enum PassageAction: Hashable {
    case note
    case quote
    case copy
    case lookUp
    case translate
    case share
    case remove
}

struct AnnotationMenu: View {
    /// The highlight's current colour, or `nil` for a plain selection.
    var current: HighlightColor?
    var hasNote = false
    /// The reading theme, so the menu sits on the page rather than on the app.
    var theme: String
    var onColor: (HighlightColor) -> Void
    var onAction: (PassageAction) -> Void
    /// Only a passage that already carries a highlight can have it removed.
    var canRemove: Bool
    /// Which way the tail points, from the anchor that placed this.
    var tail: PanelTail = .none

    /// Sized rather than measured: the anchor has to know the box before the
    /// menu is laid out, and a measured size buys nothing but a frame of the
    /// menu jumping into place.
    static let width: CGFloat = 300
    static let tailHeight: CGFloat = 8
    private static let contentHeight: CGFloat = 112
    /// What the anchor places, tail included.
    static let height: CGFloat = contentHeight + tailHeight

    private var actions: [(PassageAction, String, String)] {
        var items: [(PassageAction, String, String)] = [
            (.note, hasNote ? "Edit note" : "Note", "square.and.pencil"),
            (.quote, "Quote", "quote.opening"),
            (.copy, "Copy", "doc.on.doc"),
            (.lookUp, "Look Up", "character.book.closed"),
            (.translate, "Translate", "translate"),
            (.share, "Share", "square.and.arrow.up"),
        ]
        if canRemove {
            items.append((.remove, "Remove", "trash"))
        }
        return items
    }

    private var ink: Color { ReaderTheme.ink(theme) }

    var body: some View {
        VStack(spacing: 0) {
            colorRow
            Rectangle()
                .fill(ink.opacity(0.12))
                .frame(height: 0.5)
            actionRow
        }
        .frame(width: Self.width, height: Self.contentHeight)
        // The tail is reserved out of the frame rather than drawn over the
        // page, so the anchor's placement maths and the shape agree on where
        // the menu ends.
        .padding(tail.pointsDown ? .bottom : .top, Self.tailHeight)
        .background(
            PanelShape(tail: tail, tailHeight: Self.tailHeight)
                // The shadow rides the fill style rather than the view, so it
                // follows the tail. A `.shadow` modifier on a material-filled
                // shape falls back to the view's rectangular frame and lays a
                // hard grey band across the page under the menu.
                .fill(
                    .regularMaterial.shadow(
                        .drop(color: .black.opacity(0.24), radius: 14, y: 5)
                    )
                )
        )
        .environment(\.colorScheme, ReaderTheme.isLightPage(theme) ? .light : .dark)
    }

    /// The colours, and — on a passage already marked — the ring that says
    /// which one it is before you change it.
    private var colorRow: some View {
        HStack(spacing: 0) {
            ForEach(HighlightColor.allCases, id: \.self) { color in
                Button {
                    Haptics.select()
                    onColor(color)
                } label: {
                    Circle()
                        .fill(color.tint)
                        .frame(width: 26, height: 26)
                        .overlay(
                            Circle().strokeBorder(.white.opacity(0.28), lineWidth: 1)
                        )
                        .overlay {
                            if current == color {
                                Circle()
                                    .strokeBorder(ink, lineWidth: 2)
                                    .padding(-4)
                            }
                        }
                        .frame(maxWidth: .infinity)
                        .frame(height: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(color.label)
                .accessibilityAddTraits(current == color ? [.isSelected, .isButton] : .isButton)
            }
        }
        .padding(.horizontal, 10)
    }

    /// Scrolls rather than crowds: seven verbs at a readable size don't fit a
    /// menu narrow enough to sit beside a passage, and shrinking them to fit
    /// is how a native-looking bar starts looking like a web toolbar.
    private var actionRow: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 0) {
                ForEach(actions, id: \.0) { action, label, icon in
                    button(action, label: label, icon: icon)
                }
            }
            .padding(.horizontal, 6)
        }
        .scrollIndicators(.hidden)
        .scrollBounceBehavior(.basedOnSize)
        .frame(height: 56)
        // Fades the verbs off the edge instead of guillotining one mid-word,
        // which is the difference between "there is more" and "this is broken".
        .mask(
            LinearGradient(
                stops: [
                    .init(color: .black, location: 0),
                    .init(color: .black, location: 0.93),
                    .init(color: .clear, location: 1),
                ],
                startPoint: .leading,
                endPoint: .trailing
            )
        )
    }

    private func button(_ action: PassageAction, label: String, icon: String) -> some View {
        Button {
            Haptics.tap()
            onAction(action)
        } label: {
            VStack(spacing: 4) {
                Image(systemName: icon)
                    .font(.system(size: 15, weight: .medium))
                Text(label)
                    .font(.ui(10.5, weight: .medium))
                    .lineLimit(1)
            }
            .foregroundStyle(action == .remove ? Color.red.opacity(0.9) : ink)
            .frame(minWidth: 62)
            .padding(.vertical, 6)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Dictionary and translation

/// The system dictionary, which has no SwiftUI form.
///
/// Presented over the key window rather than as a sheet off the reader: the
/// reader owns a full-screen presentation with its own colour scheme, and a
/// `UIReferenceLibraryViewController` inside it inherits the page's theme and
/// renders grey-on-grey.
enum DictionaryLookup {
    /// A selection carries its punctuation — that is what lets a drag reach
    /// the end of a sentence — but the dictionary has no entry for `it,"`.
    private static func headword(_ term: String) -> String {
        term.trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
    }

    @MainActor
    static func present(_ term: String) {
        let word = headword(term)
        guard !word.isEmpty,
              UIReferenceLibraryViewController.dictionaryHasDefinition(forTerm: word),
              let scene = UIApplication.shared.connectedScenes
                  .compactMap({ $0 as? UIWindowScene })
                  .first(where: { $0.activationState == .foregroundActive }),
              let root = scene.keyWindow?.rootViewController
        else { return }

        var presenter = root
        while let presented = presenter.presentedViewController { presenter = presented }
        presenter.present(UIReferenceLibraryViewController(term: word), animated: true)
    }

    /// Whether the dictionary has anything to say, so the verb can be dropped
    /// rather than doing nothing when tapped.
    @MainActor
    static func hasDefinition(for term: String) -> Bool {
        let word = headword(term)
        guard !word.isEmpty else { return false }
        return UIReferenceLibraryViewController.dictionaryHasDefinition(forTerm: word)
    }
}

// MARK: - Note composer

/// Writing or editing the note attached to a highlight.
struct NoteComposer: View {
    let quote: String?
    let existing: String?
    var onSave: (String?) -> Void

    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss

    @State private var text = ""
    @State private var didLoad = false
    @FocusState private var focused: Bool

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: Spacing.lg) {
                    if let quote = quote?.nilIfBlank {
                        // The passage, set the way the journal sets an entry —
                        // you're writing about this sentence, so it should be
                        // in front of you.
                        Text(quote)
                            .font(.display(16))
                            .foregroundStyle(palette.ink1Color)
                            .lineSpacing(5)
                            .padding(.leading, 14)
                            .overlay(alignment: .leading) {
                                Capsule()
                                    .fill(palette.accentColor)
                                    .frame(width: 2.5)
                            }
                    }

                    Plate {
                        PlateField(
                            label: "Note",
                            text: $text,
                            isFirst: true,
                            multiline: true
                        )
                    }
                }
                .screenPadding()
                .padding(.top, Spacing.md)
                .padding(.bottom, 40)
            }
            .scrollIndicators(.hidden)
            .scrollDismissesKeyboard(.interactively)
            .background(ScreenBackground())
            .navigationTitle(existing?.nilIfBlank == nil ? "New note" : "Edit note")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save") {
                        onSave(text.trimmingCharacters(in: .whitespacesAndNewlines).nilIfBlank)
                        dismiss()
                    }
                }
            }
        }
        .onAppear {
            if !didLoad {
                didLoad = true
                text = existing ?? ""
            }
            focused = true
        }
    }
}

extension HighlightColor {
    var label: String {
        switch self {
        case .amber: "Amber"
        case .green: "Green"
        case .blue: "Blue"
        case .rose: "Rose"
        case .violet: "Violet"
        }
    }
}
