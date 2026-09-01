//  JournalMarkdown.swift
//  Journal bodies are authored as light markdown. This splits one into the
//  blocks and inline runs the reading face sets — the server's own vocabulary
//  minus what a phone has no room for — and renders them.

import SwiftUI

// MARK: - The pieces

/// One inline run of a journal line: prose the markdown parser handles, or a
/// `||spoiler||` region the reading face hides until it is tapped.
enum JournalSpan: Equatable {
    case prose(String)
    /// `id` is assigned in reading order across the whole entry, so a reveal
    /// survives the re-render it triggers.
    case spoiler(String, id: Int)
}

/// One block of a journal body, in the order the author wrote them.
enum JournalBlock: Equatable {
    case heading(level: Int, spans: [JournalSpan])
    case paragraph([JournalSpan])
    case bullets([[JournalSpan]])
    /// `start` is the first item's authored number — `5.` opens at five, as
    /// `<ol start="5">` does. Later markers are ignored, per CommonMark.
    case numbered([[JournalSpan]], start: Int)
    case quote([JournalSpan])
    /// Fenced source, kept verbatim — no inline parse, no spoilers.
    case code(String)
    case rule
}

// MARK: - The splitter

/// Markdown for journal bodies. Line-based rather than a real CommonMark
/// parser: the server owns the canonical render, and this only has to agree
/// with it on the constructs a phone actually shows. Images, tables and task
/// lists are not among them — their source falls through as prose.
enum JournalMarkdown {
    /// URL scheme the spoiler reveal rides on. `Text` has no per-run gesture,
    /// but it does route link taps through `OpenURLAction` — so a masked span
    /// is a link the body intercepts rather than opens.
    static let revealScheme = "omnibus-spoiler"

    /// Split a body into blocks, numbering every spoiler in reading order.
    static func blocks(_ md: String) -> [JournalBlock] {
        var splitter = Splitter()
        // Normalized first: splitting on a newline *set* would read CRLF as two
        // breaks, and the blank line between them ends the block.
        let normalized = md
            .replacingOccurrences(of: "\r\n", with: "\n")
            .replacingOccurrences(of: "\r", with: "\n")
        for line in normalized.components(separatedBy: "\n") {
            splitter.take(line)
        }
        return splitter.finish()
    }

    /// Split one run of inline markdown on its `||spoiler||` pairs.
    ///
    /// Pairing is **line-local**: a marker never pairs across a newline, so an
    /// unbalanced one cannot invert every line after it (#2366 — the server's
    /// own renderer is being brought to the same rule). An unterminated marker
    /// is left literal, as it is there.
    static func spans(_ text: String, from next: inout Int) -> [JournalSpan] {
        var out: [JournalSpan] = []

        func prose(_ chunk: String) {
            guard !chunk.isEmpty else { return }
            if case .prose(let head) = out.last {
                out[out.count - 1] = .prose(head + chunk)
            } else {
                out.append(.prose(chunk))
            }
        }

        for (index, line) in text.components(separatedBy: "\n").enumerated() {
            if index > 0 { prose("\n") }
            var rest = Substring(line)
            while let open = rest.range(of: "||") {
                let after = rest[open.upperBound...]
                guard let close = after.range(of: "||") else { break }
                prose(String(rest[..<open.lowerBound]))
                out.append(.spoiler(String(after[..<close.lowerBound]), id: next))
                next += 1
                rest = after[close.upperBound...]
            }
            prose(String(rest))
        }
        return out
    }

    /// One line with every spoiler region replaced by censor bars. For the
    /// always-visible surfaces — a row excerpt can never leak one.
    static func masked(_ line: String) -> String {
        var ignored = 0
        return spans(line, from: &ignored).map { span in
            switch span {
            case .prose(let text): text
            case .spoiler: "\u{2588}\u{2588}\u{2588}"
            }
        }.joined()
    }

    /// One run of inline markdown — bold, italic, strikethrough, code, links.
    /// `inlineOnlyPreservingWhitespace` keeps the line breaks the splitter has
    /// already reduced to breaks *within* a block.
    static func inline(_ source: String) -> AttributedString {
        (try? AttributedString(
            markdown: source,
            options: .init(
                interpretedSyntax: .inlineOnlyPreservingWhitespace,
                failurePolicy: .returnPartiallyParsedIfPossible
            )
        )) ?? AttributedString(source)
    }

    /// The block as assistive tech should hear it: markup resolved, and every
    /// still-hidden span named rather than read out. Masking a run by colour
    /// hides it from exactly one audience and not from the other — VoiceOver
    /// speaks the text under the bar — so the label has to censor it too.
    static func spoken(_ spans: [JournalSpan], revealed: Set<Int>) -> String {
        spans.map { span in
            switch span {
            case .prose(let text):
                String(inline(text).characters)
            case .spoiler(let text, let id):
                revealed.contains(id) ? String(inline(text).characters) : "spoiler, hidden"
            }
        }.joined()
    }

    /// The spans in one block that are still hidden — what a reveal-all
    /// accessibility action opens, and whether to offer one at all.
    static func hiddenIDs(_ spans: [JournalSpan], revealed: Set<Int>) -> Set<Int> {
        var out: Set<Int> = []
        for case .spoiler(_, let id) in spans where !revealed.contains(id) {
            out.insert(id)
        }
        return out
    }

    /// The link a masked spoiler carries, and the id back out of one.
    static func revealURL(_ id: Int) -> URL? { URL(string: "\(revealScheme)://\(id)") }

    static func revealID(_ url: URL) -> Int? {
        guard url.scheme == revealScheme else { return nil }
        return url.host.flatMap(Int.init)
    }
}

// MARK: - Line classification

extension JournalMarkdown {
    /// Accumulates lines into blocks. A struct rather than a pile of free
    /// functions so the pending-block state and the spoiler counter — which
    /// must advance in reading order — stay in one place.
    private struct Splitter {
        /// The block still taking lines. Items hold their own lines so a
        /// wrapped list item keeps its breaks: the server promotes a soft break
        /// to a hard one, so a newline the author can see stays visible.
        private enum Pending {
            case none
            case paragraph([String])
            case bullets([[String]])
            case numbered([[String]], Int)
            case quote([String])
        }

        private var out: [JournalBlock] = []
        private var pending = Pending.none
        private var nextSpoiler = 0
        /// The open fence's marker run, when inside a fenced code block.
        private var fence: String?
        private var code: [String] = []

        mutating func take(_ line: String) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            if let open = fence {
                if Self.closesFence(trimmed, open: open) {
                    out.append(.code(code.joined(separator: "\n")))
                    fence = nil
                    code = []
                } else {
                    code.append(line)
                }
                return
            }
            if let opened = Self.fenceMarker(trimmed) {
                flush()
                fence = opened
                return
            }
            if trimmed.isEmpty {
                flush()
                return
            }
            // Setext before the thematic break: `---` under a paragraph
            // underlines it rather than ruling below it.
            if case .paragraph(let lines) = pending, let level = Self.setextLevel(trimmed) {
                pending = .none
                let spans = inline(lines.joined(separator: "\n"))
                out.append(.heading(level: level, spans: spans))
                return
            }
            if Self.isRule(trimmed) {
                flush()
                out.append(.rule)
                return
            }
            if let heading = Self.atxHeading(trimmed) {
                flush()
                let spans = inline(heading.text)
                out.append(.heading(level: heading.level, spans: spans))
                return
            }
            if let item = Self.bulletItem(trimmed) {
                if case .bullets(var items) = pending {
                    items.append([item])
                    pending = .bullets(items)
                } else {
                    flush()
                    pending = .bullets([[item]])
                }
                return
            }
            if let item = Self.numberedItem(trimmed) {
                if case .numbered(var items, let start) = pending {
                    items.append([item.text])
                    pending = .numbered(items, start)
                } else {
                    flush()
                    pending = .numbered([[item.text]], item.number)
                }
                return
            }
            if let text = Self.quoteLine(trimmed) {
                if case .quote(var lines) = pending {
                    lines.append(text)
                    pending = .quote(lines)
                } else {
                    flush()
                    pending = .quote([text])
                }
                return
            }
            // A plain line under an open list or quote is a lazy continuation of
            // its last item, not a new paragraph.
            switch pending {
            case .bullets(var items):
                items[items.count - 1].append(trimmed)
                pending = .bullets(items)
            case .numbered(var items, let start):
                items[items.count - 1].append(trimmed)
                pending = .numbered(items, start)
            case .quote(var lines):
                lines.append(trimmed)
                pending = .quote(lines)
            case .paragraph(var lines):
                lines.append(trimmed)
                pending = .paragraph(lines)
            case .none:
                pending = .paragraph([trimmed])
            }
        }

        mutating func finish() -> [JournalBlock] {
            // An unclosed fence still holds real text; keep it rather than drop
            // the tail of the entry.
            if fence != nil {
                if !code.isEmpty { out.append(.code(code.joined(separator: "\n"))) }
                fence = nil
                code = []
            }
            flush()
            return out
        }

        private mutating func flush() {
            let block: JournalBlock?
            switch pending {
            case .none:
                block = nil
            case .paragraph(let lines):
                block = .paragraph(inline(lines.joined(separator: "\n")))
            case .bullets(let items):
                block = .bullets(items.map { inline($0.joined(separator: "\n")) })
            case .numbered(let items, let start):
                block = .numbered(
                    items.map { inline($0.joined(separator: "\n")) }, start: start)
            case .quote(let lines):
                block = .quote(inline(lines.joined(separator: "\n")))
            }
            pending = .none
            if let block { out.append(block) }
        }

        private mutating func inline(_ text: String) -> [JournalSpan] {
            JournalMarkdown.spans(text, from: &nextSpoiler)
        }

        // MARK: Line shapes

        /// The backtick or tilde run opening a fence, if this line is one —
        /// the whole run, not just its first three. CommonMark lets a longer
        /// fence carry ``` inside it, and only a run at least as long closes
        /// one, so the length has to survive to the closing check.
        static func fenceMarker(_ line: String) -> String? {
            for marker: Character in ["`", "~"] {
                let run = line.prefix { $0 == marker }
                if run.count >= 3 { return String(run) }
            }
            return nil
        }

        /// Whether this line closes the open fence: a bare run of the same
        /// marker, at least as long as the one that opened it. A shorter run,
        /// or one with an info string after it, is content.
        static func closesFence(_ line: String, open: String) -> Bool {
            guard let marker = open.first else { return false }
            let run = line.prefix { $0 == marker }
            return run.count >= open.count && run.count == line.count
        }

        /// `---`, `***`, `___` — three or more of one marker and nothing else.
        static func isRule(_ line: String) -> Bool {
            guard let first = line.first, "-*_".contains(first) else { return false }
            let body = line.filter { !$0.isWhitespace }
            return body.count >= 3 && body.allSatisfy { $0 == first }
        }

        /// A setext underline: an unbroken run of `=` (h1) or `-` (h2). `- - -`
        /// is a thematic break, so internal whitespace disqualifies it.
        static func setextLevel(_ line: String) -> Int? {
            guard let first = line.first, first == "=" || first == "-" else { return nil }
            guard line.allSatisfy({ $0 == first }) else { return nil }
            return first == "=" ? 1 : 2
        }

        /// `# Heading` through `###### Heading`, closing `#`s dropped.
        static func atxHeading(_ line: String) -> (level: Int, text: String)? {
            let hashes = line.prefix { $0 == "#" }
            guard (1...6).contains(hashes.count) else { return nil }
            let rest = line.dropFirst(hashes.count)
            guard rest.isEmpty || rest.first == " " else { return nil }
            let text = rest.trimmingCharacters(in: .whitespaces)
            // A trailing `#` run only *closes* the heading when a space runs up
            // to it, so `# C#` keeps its sharp rather than losing it.
            let closing = text.reversed().prefix { $0 == "#" }.count
            guard closing > 0 else { return (hashes.count, text) }
            let head = String(text.dropLast(closing))
            guard head.isEmpty || head.last == " " else { return (hashes.count, text) }
            return (hashes.count, head.trimmingCharacters(in: .whitespaces))
        }

        /// `- item`, `* item`, `+ item`. The space is required, so `---` stays a
        /// rule and `*emphasis*` stays prose.
        static func bulletItem(_ line: String) -> String? {
            guard let first = line.first, "-*+".contains(first) else { return nil }
            let rest = line.dropFirst()
            guard rest.first == " " else { return nil }
            return String(rest.drop { $0 == " " })
        }

        /// `1. item` / `1) item`, up to the nine digits CommonMark allows —
        /// with the number, which the first item of a list opens it at.
        static func numberedItem(_ line: String) -> (number: Int, text: String)? {
            let digits = line.prefix(while: \.isNumber)
            guard (1...9).contains(digits.count), let number = Int(digits) else { return nil }
            let rest = line.dropFirst(digits.count)
            guard let delimiter = rest.first, delimiter == "." || delimiter == ")" else {
                return nil
            }
            let body = rest.dropFirst()
            guard body.first == " " else { return nil }
            return (number, String(body.drop { $0 == " " }))
        }

        /// `> quoted`, with the one optional space after the marker.
        static func quoteLine(_ line: String) -> String? {
            guard line.first == ">" else { return nil }
            let rest = line.dropFirst()
            return String(rest.first == " " ? rest.dropFirst() : rest)
        }
    }
}

// MARK: - The reading face

/// A journal body set as real blocks — paragraphs, lists, headings, quotes —
/// rather than one run-on run. `||spoilers||` render as bars; where they are
/// revealable, tapping one opens it and tapping it again puts it back.
struct JournalBody: View {
    let md: String
    /// Whether a spoiler here can be tapped open. False inside the all-entries
    /// sheet, where the whole card is a button and the row's own tap — which
    /// opens the entry in the drawer — has to win.
    var revealable: Bool = true

    @Environment(\.palette) private var palette
    @State private var revealed: Set<Int> = []

    /// Reveal is a toggle, not a latch: having read a span, you can hide it
    /// again before handing the phone over.
    static func toggling(_ id: Int, in revealed: Set<Int>) -> Set<Int> {
        var next = revealed
        if next.remove(id) == nil { next.insert(id) }
        return next
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 13) {
            ForEach(Array(JournalMarkdown.blocks(md).enumerated()), id: \.offset) { _, block in
                view(for: block)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .environment(\.openURL, OpenURLAction { url in
            // Links the author wrote still open; only the reveal scheme is ours
            // to swallow.
            guard let id = JournalMarkdown.revealID(url) else { return .systemAction }
            withAnimation(Motion.snap) { revealed = Self.toggling(id, in: revealed) }
            return .handled
        })
    }

    @ViewBuilder
    private func view(for block: JournalBlock) -> some View {
        switch block {
        case .heading(let level, let spans):
            spoilerSafe(
                text(spans, ink: palette.ink0Color)
                    .font(.display(level <= 1 ? 25 : level == 2 ? 21 : 18, weight: .semibold))
                    .foregroundStyle(palette.ink0Color)
                    .lineSpacing(2)
                    .frame(maxWidth: .infinity, alignment: .leading),
                spans
            )
        case .paragraph(let spans):
            prose(spans)
        case .bullets(let items):
            list(items) { _ in "\u{2022}" }
        case .numbered(let items, let start):
            list(items) { "\(start + $0)." }
        case .quote(let spans):
            prose(spans)
                .padding(.leading, 13)
                .overlay(alignment: .leading) {
                    Capsule().fill(palette.accentColor.opacity(0.5)).frame(width: 2)
                }
        case .code(let source):
            Text(source)
                .font(.monoUI(12.5))
                .foregroundStyle(palette.ink1Color)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(11)
                .background(
                    RoundedRectangle(cornerRadius: Radius.md, style: .continuous)
                        .fill(palette.bg2Color)
                )
        case .rule:
            // Not `Hairline`: that is a row separator at line-2/0.5pt, which an
            // authored break disappears into. A `---` is a deliberate mark.
            Rectangle()
                .fill(palette.lineColor)
                .frame(height: 1)
                .padding(.vertical, 5)
        }
    }

    private func prose(_ spans: [JournalSpan]) -> some View {
        spoilerSafe(
            text(spans, ink: palette.ink1Color)
                .font(.display(18))
                .foregroundStyle(palette.ink1Color)
                .lineSpacing(6)
                .multilineTextAlignment(.leading)
                .frame(maxWidth: .infinity, alignment: .leading),
            spans
        )
    }

    /// A block holding a masked span speaks a censored label — the colour
    /// mask means nothing to VoiceOver — and, where its spans can be opened,
    /// offers one rotor action to open them, since the per-span reveal links
    /// the label replaces are no longer reachable.
    @ViewBuilder
    private func spoilerSafe(_ view: some View, _ spans: [JournalSpan]) -> some View {
        let hidden = JournalMarkdown.hiddenIDs(spans, revealed: revealed)
        if hidden.isEmpty {
            view
        } else if revealable {
            view
                .accessibilityLabel(JournalMarkdown.spoken(spans, revealed: revealed))
                .accessibilityAction(named: "Reveal spoilers") {
                    withAnimation(Motion.snap) { revealed.formUnion(hidden) }
                }
        } else {
            view.accessibilityLabel(JournalMarkdown.spoken(spans, revealed: revealed))
        }
    }

    /// A list hung on its markers, so a wrapped line lines up under the text
    /// rather than under the bullet.
    private func list(_ items: [[JournalSpan]], marker: (Int) -> String) -> some View {
        // Resolved up front: `ForEach`'s content escapes, and the caller's
        // marker closure does not.
        let markers = items.indices.map(marker)
        return VStack(alignment: .leading, spacing: 6) {
            ForEach(items.indices, id: \.self) { index in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(markers[index])
                        .font(.monoUI(11))
                        .foregroundStyle(palette.ink3Color)
                        .frame(minWidth: 12, alignment: .trailing)
                    prose(items[index])
                }
            }
        }
    }

    /// The inline runs of one block, spoilers masked or open. `ink` is the
    /// block's own text colour, which an open spoiler has to restate: it keeps
    /// its reveal link so a second tap re-hides it, and a link left to itself
    /// would draw in the tint colour rather than as prose.
    private func text(_ spans: [JournalSpan], ink: Color) -> Text {
        spans.reduce(Text(verbatim: "")) { line, span in line + piece(span, ink: ink) }
    }

    private func piece(_ span: JournalSpan, ink: Color) -> Text {
        switch span {
        case .prose(let source):
            return Text(JournalMarkdown.inline(source))
        case .spoiler(let source, let id):
            var run = JournalMarkdown.inline(source)
            if revealed.contains(id) {
                run.foregroundColor = ink
            } else {
                // Ink the colour of its own bar: the region keeps the width it
                // will have open, so a reveal never reflows the paragraph.
                run.foregroundColor = palette.bg3Color
                run.backgroundColor = palette.bg3Color
            }
            if revealable { run.link = JournalMarkdown.revealURL(id) }
            return Text(run)
        }
    }
}
