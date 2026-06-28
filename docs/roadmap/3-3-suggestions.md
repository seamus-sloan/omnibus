# F3.3 — Suggestions

**Phase 3 · Personalization** · **Priority:** P3

"More like this" on the book detail page, powered by Hardcover community-list co-occurrence.

## Objective

Below the metadata on a book detail page, show a single **"Readers also enjoyed"** strip of up to **10** books that real readers shelve alongside the current one — drawn from Hardcover and deliberately steered toward *new* discoveries:

- **Different author** — never suggest another book by an author already on the page.
- **Different series** — never suggest another entry in a series the book belongs to.
- **Entry points only** — only series-starters (`position ≤ 1`) or standalones, so a reader is never dropped into the middle of a series.

## Why Hardcover (and nothing else)

We tested the field before committing:

- **Goodreads** — API retired since 2020; keys now 403. Dead.
- **StoryGraph / Fable** — the best modern recommendation engines, but neither exposes a public API to call.
- **Google Books / OpenLibrary** — active APIs, but no recommendation/similarity surface; you'd be back to genre-guessing.
- **Big Book API** — has a turnkey `/{id}/similar` endpoint, but its OpenLibrary-derived catalog is low quality (live testing could not resolve 3 of 4 modern titles, and surfaced authors literally named "Test Author"), and it's paid past ~25 books/day.
- **Hardcover** — active, free GraphQL API; a **single account-level token** (no per-user key management); and, crucially, rich crowd-curated **lists** we can mine for co-occurrence.

Hardcover is the only source that is callable, free of per-user credentials, and carries a signal strong enough to produce genuine read-alikes.

## Approach — list co-occurrence, not genre overlap

Hardcover has no recommendations endpoint, so we compose one from list data. The signal that works is **collaborative filtering over curated lists** — "what else do people who shelved this book shelve?" (Genre-tag overlap was tried and rejected: it returns a genre's bestsellers — e.g. *A Clash of Kings* for any epic-fantasy book — not true read-alikes.)

Per book, against `https://api.hardcover.app/v1/graphql`:

1. **Resolve** the library book to a Hardcover book — match on an ISBN from `book_identifiers` first, fall back to a title+author `search`, and pick the most-read canonical edition. Resolution matters: a naive title search matched a *"Fourth Wing Summary"* knock-off and returned garbage.
2. **Collect lists** containing it via `list_books`, restricted to curated lists (`books_count` 5–150, public) so giant dumping-ground shelves don't dominate.
3. **Rank** every other book by how many of those lists it also appears on.
4. **Filter** out same-author, same-series, and any book not first in its series (`book_series.position ≤ 1`; standalones pass).
5. **Return** the top 10 survivors.

Because the same-author/same-series filter removes a lot — an author-heavy book like *Wind and Truth* sheds its entire top-of-list — the candidate pool is taken deep (top ~70 co-listed) before trimming to 10.

## Validated

Run live against the Hardcover API, the engine produces clean cross-author entry-point lists:

- **Fourth Wing** → The Invisible Life of Addie LaRue, A Court of Thorns and Roses, Tomorrow and Tomorrow and Tomorrow, Ninth House, The Love Hypothesis, …
- **Two Twisted Crowns** → Fourth Wing, A Fate Inked in Blood, The Atlas Six, Legendborn, Throne of Glass, …
- **Throne of the Fallen** → Fourth Wing, A Court of Thorns and Roses, Divine Rivals, Powerless, …
- **Wind and Truth** → The Eye of the World, Project Hail Mary, Assassin's Apprentice, Red Rising, All Systems Red, …

## Technical considerations

- **Config:** one server-wide `HARDCOVER_API_KEY` (Bearer) in env — never per-user, never in the DB. Backend-only; the token must not reach the WASM client. Hardcover limits: 60 req/min, 30s per query, token auto-expires yearly (resets Jan 1).
- **Worker + cache:** the ~2–3 Hardcover calls per book run through the [F0.5 worker](0-5-background-worker.md), never on the detail-page request path. Results cache per book with a 30-day TTL, three-state like the author-photo resolver (fresh → serve, stale/absent → enqueue refetch, clean miss → sticky negative marker).
- **Relevance floor:** quality tracks how many curated lists a book appears on. Obscure books with few list appearances yield thin results — store the co-listing count and hide low-confidence rows rather than show filler.
- **Series-position dependency:** the "entry points only" filter trusts Hardcover's `book_series.position`. Bad or missing position data is the main correctness risk.

## Dependencies

- [F0.1 Schema refactor](0-1-schema-refactor.md) — `book_identifiers` for ISBN-based resolution.
- [F0.5 Background worker](0-5-background-worker.md) — off-request fetching + caching.

## Changes from prior versions

- **v1 → v2:** dropped Hardcover over a per-user API-key concern; planned local signals + OpenLibrary instead.
- **v2 → v3 (current):** **Hardcover is now the sole source.** The per-user-key objection was wrong — Hardcover issues one account-level token, so a single env var covers the whole instance. Local same-author/same-series signals and the OpenLibrary "readers also enjoyed" idea are **dropped**: live testing showed Hardcover list co-occurrence is a materially better read-alike signal, and the feature now deliberately *excludes* same-author/same-series rather than showcasing them. Big Book API was evaluated and rejected (poor catalog).

---

[← Back to roadmap summary](0-0-summary.md)
