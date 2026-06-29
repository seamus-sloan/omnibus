# F3.2b — Rich journal editor

**Phase 3 · Personalization** · **Priority:** P3

A WYSIWYG editing experience layered on top of F3.2's plain-markdown journals.

## Objective

[F3.2](3-2-ratings-journaling.md) ships journals with a plain markdown textarea +
Write/Preview toggle and server-side markdown rendering. F3.2b upgrades the
authoring experience to match the design (`screens/journal.jsx`,
`InlineJournalEditor` in `screens/book-detail.jsx`) **without changing the stored
format** (still markdown):

- **Formatting toolbar** — bold, italic, strikethrough, H1/H2, blockquote,
  bullet/ordered/checklist, inline code, link, and a **spoiler** button. Buttons
  wrap/insert the equivalent markdown (incl. the `||spoiler||` syntax F3.2
  defines) so the persisted body stays plain markdown.
- **Drafts + autosave** — "Save draft" vs "Publish", an "auto-saved Ns ago"
  indicator, and a `status` column (`draft` | `published`) so drafts are
  excluded from the public feed until published.
- **Embedded images** — inline figures with captions. Depends on an upload
  surface (F5.3) for storing user-supplied images.
- **Insert from highlights** — drag/insert a saved highlight as a blockquote
  ("saved from highlights"). Depends on highlights (F2.4b).
- **Full-page editor** — the standalone long-form journal screen
  (`screens/journal.jsx`) with a right-rail entry list + highlights panel, for
  writing longer entries than the inline composer suits.

## User / business value

The inline markdown composer covers quick notes; serious journalers want a
richer surface. Keeping the stored format as markdown means this is purely an
editing-UX upgrade — no data migration, no change to rendering or durability.

## Technical considerations

- **Stored format is unchanged** — toolbar actions and the WYSIWYG view both
  serialize to the same markdown F3.2 persists and renders
  (`db::journals::markdown::render`). No schema change except a `status`
  (`draft`/`published`) column for drafts, defaulting to `published` so existing
  rows are unaffected; the public list filters to `published`.
- **Autosave** reuses the F3.2 create/update endpoints (debounced); drafts are
  per-user-private until published.
- **Editor library** — prefer a lightweight markdown-aware editor over a heavy
  WYSIWYG framework; the toolbar can be hand-rolled buttons over the existing
  textarea before adopting any dependency. Mind SSR/WASM hydration parity
  (rule 07) — editor wiring belongs in a post-mount effect, not the rsx body.

## Dependencies

- [F3.2 Ratings & journaling](3-2-ratings-journaling.md) — base journals.
- [F2.4b Reader interactive features](2-4b-reader-interactive.md) — highlights,
  for insert-from-highlights.
- [F5.3 Uploads](5-3-uploads.md) — image storage for embedded figures.
- Adjacent: [F5.7 Journal & quote cards](5-7-journal-quote-cards.md).

---

[← Back to roadmap summary](0-0-summary.md)
