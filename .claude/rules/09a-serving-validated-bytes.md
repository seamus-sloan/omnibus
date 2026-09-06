# 09a — Serving validated bytes

Companion to [09-content-validators.md](09-content-validators.md), which owns
what a validator *is* and how a client compares one. This file is the server
half: how a response carries a validator, and what an endpoint serving bytes
off disk must do with the conditional headers a client sends back.

Split out of rule 09 rather than written fresh — the two halves have different
audiences. A client author needs the other file; whoever adds a byte-serving
route needs this one.

## Strong, not weak

Both validators are emitted as **strong** entity-tags (no `W/` prefix), which
is what nginx and Apache do with their own stat-derived defaults. This is not
cosmetic: RFC 9110 §13.1.5 requires strong comparison for `If-Range`, so a
weak tag would make resume permanently impossible — every resume would
restart from zero.

## Every content endpoint routes through `serve_file`

An endpoint that serves bytes from disk calls `backend::serve_file` (or
`serve_download`, which is the same thing plus a `Content-Disposition`).
Do not hand the job to `tower_http::ServeFile`, and do not evaluate
preconditions anywhere else. Three properties depend on that, and each has
already been got wrong once:

- **One open handle for both the validator and the bytes.** Deriving a
  validator from a *path* and then re-opening that path to stream it leaves
  a window in which an atomic replace lands between the two — an `If-Range`
  that matched the old file, served from the new one, which is precisely the
  splice all of this exists to prevent. `conditional::open` returns the
  handle its validator came from and `conditional::serve` streams from that
  same handle; on Unix a `rename` cannot move an open inode, so the
  representation is pinned for the life of the response.
- **All five preconditions settled in one place**, in RFC 9110 §13.2.2
  order: `If-Match` → `If-Unmodified-Since` → `If-None-Match` →
  `If-Modified-Since` → `Range`/`If-Range`. Splitting them across two layers
  is not a partial implementation, it is a wrong one: a stale
  `If-None-Match` alongside a matching `If-Modified-Since` must produce the
  full body, because step 4 runs only when step 3's header is *absent*.
  Leaving the date conditions to a file service with its own validator
  answers 304 to a client that just said its copy was out of date.
- **416 is an answer, not an error.** An unsatisfiable range is a legitimate
  416 carrying `Content-Range: bytes */{len}`. Collapsing it into 404 tells
  a resuming client the book disappeared and discards the length it needs to
  restart. Only a genuinely missing file is a 404.

`tower-http` supplies an `ETag` and `If-None-Match` on its own, but has **no
`If-Range` support at all** and opens by path — so it can satisfy none of the
three.

## `ETag` and `Vary` travel together

An `ETag` on a route that authenticates by cookie *or* bearer *or* `?token=`
needs `Vary: Cookie, Authorization` (`MEDIA_VARY`), on the **304 as well as
the 200**. Without it a shared cache can hand one user's 304 — hence their
copy of the book — to a differently-authenticated request on the same URL.
`with_media_cache_policy` and `not_modified` exist so the two paths cannot
drift apart.

## Out of scope

- What the two validators are, how they are derived, and the three-valued
  comparison a client stores — [09-content-validators.md](09-content-validators.md).
- What a client may *queue* while offline — see
  [08-offline-writes.md](08-offline-writes.md).
