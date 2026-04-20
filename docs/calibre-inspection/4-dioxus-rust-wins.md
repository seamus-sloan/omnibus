# 4. Where Dioxus / Rust wins

Given Omnibus' stack (axum + sqlx + Dioxus fullstack):

- **sqlx vs SQLAlchemy.** sqlx emits raw SQL with compile-time checks; the JOIN-over-`.any()` optimization PR [#3476](https://github.com/janeczku/calibre-web/pull/3476) had to retrofit is *the default* on sqlx. Collecting into serializable structs (`#[derive(FromRow)]`) skips ORM object graph hydration, which is a large fraction of SQLAlchemy's per-row cost. Expect Omnibus to not need PR #3476's heroics at all.

- **No GIL.** Cover extraction, EPUB OPF parsing, and thumbnail resize (the `image` crate) can truly parallelize across cores during library scan. Calibre-Web's single `WorkerThread` becomes a `tokio::task::JoinSet` bounded by a `Semaphore`. `ebook-convert` is still subprocess-bound — but you can run N of them concurrently without starving the web path.

- **SSR + WASM hydration.** Dioxus fullstack lets you render the book grid server-side (no blank page, good for low-end devices), then hydrate for interactive filter/sort/search. Calibre-Web full-reloads the page for every filter click — Jinja has no concept of "add another tag filter" without a new HTTP round trip. Dioxus signals can re-filter a pre-fetched `Vec<BookListItem>` in-memory without touching the network.

- **Embedded FTS5, always.** Create the `books_fts` virtual table at startup in `initialize_schema` with triggers on `books` (insert/update/delete) so the index stays in sync. Calibre-Web only gets FTS5 when Calibre-desktop created it; Omnibus can guarantee it, so the fast path is the only path.

- **Streaming responses.** axum's `StreamBody` + server-sent events or chunked JSON can deliver the first 50 books while the next 50 are still being assembled — useful for a Kobo-like sync endpoint that wants to respect a client's read buffer instead of the hard `SYNC_ITEM_LIMIT=100` cutoff.

- **Native image processing.** Swap Wand+ImageMagick for the `image` crate + `resvg` (for SVG covers). Deterministic binary, no shell out, no ImageMagick policy file. PDF first-page extraction can use `pdfium-render` or `mupdf` bindings for speed.

- **Memory & startup.** A cold cargo-built binary is ~20–40 MB RSS vs 70–100 MB for the Python stack — matters on Pi / Synology deployments that Calibre-Web targets.

- **Prefetch + Dioxus Router.** Link prefetch on hover + route-level code splitting gives sub-100ms navigations between library sections once hydrated, which Calibre-Web cannot do at all.

---

[← Performance pain points](3-performance-pain-points.md) · [Next: schema details →](5-schema-details.md)
