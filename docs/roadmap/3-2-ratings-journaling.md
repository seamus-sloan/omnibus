# F3.2 — Ratings & journaling

**Phase 3 · Personalization** · **Priority:** P2

Per-user star ratings and free-form journal entries per book.

## Objective

1-5 star rating per user per book, plus a free-form markdown journal (multiple dated entries) on each book detail page.

## User / business value

"My library, with my notes" is the pitch for self-hosted over bookstore apps. Rating data also feeds [F3.3 Suggestions](3-3-suggestions.md).

## Technical considerations

- Two tables: `user_ratings(user_id, book_id, stars, updated_at)` and `user_journal_entries(id, user_id, book_id, body_md, created_at)`.
- Render journal entries with a server-side markdown renderer (`pulldown-cmark`) + sanitization — never trust raw HTML.
- Ratings UI lives in the book detail page's pre-allocated slot from [F1.4](1-4-book-detail.md).

## Dependencies

- [F0.1 Schema refactor](0-1-schema-refactor.md).
- [F0.3 Auth](0-3-auth.md).

---

[← Back to roadmap summary](0-0-summary.md)
