# 09 — Content validators

A **content validator** is how a client learns that bytes it already holds
have changed. Omnibus publishes two, deliberately different, and the
distinction is the whole rule: one guards a *notification*, the other guards
a *file*.

| | Source | Granularity | Answers |
|---|---|---|---|
| **Response `ETag`** | the served file's stat, at request time | `(inode, mtime secs.nanos, size)` | `If-None-Match`, `If-Range` |
| **Wire etag** (`BookFileInfo.etag`) | `book_files` columns | `(mtime_epoch, size_bytes)`, seconds | "is my downloaded copy stale?" |

Do not collapse them into one value. A miss on the response validator
splices a file; a miss on the wire validator merely delays a chip in the UI.

## Derived, never stored

`omnibus_shared::file_etag` derives the wire validator from the
`(mtime_epoch, size_bytes)` pair the scanner already records — the **same
pair the reindex diff keys on** (`db/src/indexer/mod.rs`). That is the point:
"the validator moved" and "the indexer classified this file Changed" are one
statement, so they cannot disagree.

- **Never add a stored validator column.** It would be a second copy of a
  fact that every `book_files` insert and backfill site must remember to
  bump; one missed write path drifts silently. Deriving cannot.
- **Never add a manual version counter**, for the same reason.
- **Never hash file contents** to build it. Byte-hashing a library of
  multi-hundred-MB audiobooks on every scan is not a validator, it's a
  rescan. (Cover and comic-page handlers *do* hash — they have read the bytes
  in to serve them, so it's free there. Thumbs are the exception: `thumb_etag`
  derives from `(book_id, size, last_modified_epoch, encoder_version)`, so its
  304 path never opens the file. **Known residual:** a re-encode that changes
  the bytes without moving that key is not detected as stale.)
- `(0, 0)` is the never-observed sentinel and must map to `None`, so the
  one-time stat backfill never reads as a content change on a device
  holding a download.

## Why the response validator carries the inode

A stat-derived tag's real weakness is a replacement that *preserves* the
timestamp: `rsync -t`, `cp -p`, a restore from backup. Nanosecond precision
does not help there. But almost every such tool writes a temp file and
renames it over the target, which moves the inode even when the mtime comes
along unchanged — so including it closes the dominant case. This is Apache's
historic `FileETag INode MTime Size`; it dropped `INode` only because it
breaks multi-server clusters, which a self-hosted single instance is not.

**Known residual:** an explicit in-place overwrite that preserves both length
and timestamp (`rsync --inplace`, `dd`) still slips through. No stat-derived
validator catches that.

`frontend/src/offline/downloads/verify.rs` backstops it from the other end,
and how well is **format-dependent** — say which when you touch it:

- **EPUB** is CRC-backed. Every member is read to EOF, which checks the
  CRC-32 the central directory recorded, so any corruption is caught —
  including a same-length splice that leaves the archive's structure and all
  its offsets perfectly valid. An offsets-only check calls that intact; only
  the CRCs give it away.
- **M4B/M4A** is structural only. The box chain must tile the file, which
  catches truncation, garbage, and a splice that changes a box size — but a
  same-length splice *inside* `mdat` leaves every header untouched and is
  undetectable. The format carries no checksum to catch it with.
- **MP3** has no container and reports `Unverifiable`.

Do not describe this as "verifying the file parses". It is a CRC check for
one format and a structure walk for another, and the audio gap is real.

## Resume state must be durable and provable

Two rules for anything that resumes a partial download, both learned the
hard way:

- **Persist the response `ETag` when the headers arrive, not when the
  transfer returns.** A process kill or a dropped task runs no code after
  the transfer — and those, not clean error returns, are how a download on
  a phone actually gets interrupted. A `.part` whose tag never reached disk
  resumes with a bare `Range`, which is the splice this rule exists to
  prevent.
- **Never delete the copy the reader has before the replacement lands.**
  A finished file is superseded by an atomic rename at the end of a
  successful fetch; until then it is the only copy on the device. Deleting
  it up front buys nothing and turns any later failure — network, auth,
  integrity — into a book that used to work and now doesn't.
- **Never restamp bytes whose provenance you cannot prove.** Carrying a
  previously-fetched part into a new attempt and assigning it the current
  validator turns "no idea where these bytes came from" into "these bytes
  are current" — and lets a part from one edition sit in the same audiobook
  as parts from another with nothing downstream ever reporting it. Reuse
  only on a *proven* match; discard and refetch when the current file has a
  validator the part cannot be shown to match. The one exception is when
  neither side has a validator: nothing has been learned, so refusing to
  resume would only break downloads for a row the scanner has never stat'd.

## Clients: snapshot, then compare

Both offline clients snapshot `BookFileInfo.etag` when a download starts and
compare it against a later metadata refresh — `PlannedFile.source_etag` in
`frontend/src/offline/downloads.rs`, `DownloadRecord.sourceEtag` in
`omnibus-ios/omnibus/Offline/OfflineStore.swift`. Two rules hold on both sides:

- **The comparison is three-valued.** A missing validator on either side means
  *can't tell*, which is not the same as *not stale*. A renderer may collapse
  it to "not stale"; anything that **stores** the answer must not, or a read
  that couldn't tell will clear a flag a real comparison had set. Only the
  per-book detail read carries `book_files` — the library listing projection
  has none — so this case is routine, not theoretical.
- **The compared file is the one the server would serve**: lowest ordinal of
  a format the endpoint actually *serves*, matching `db::book_file_path`'s
  `ORDER BY bf.ordinal LIMIT 1`. Two ways to get this wrong, and both have
  happened: comparing against the wrong row of a two-edition book, and
  matching on the formats a library can *contain* rather than the narrow set
  a download pulls — `/api/ebooks/{uuid}/file` serves the EPUB, else the CBZ
  for a comic-only book; the audiobook routes M4B/M4A/MP3 alone. A mixed
  PDF/EPUB book that snapshots the PDF reports staleness about a file the
  device doesn't hold and misses every change to the one it does.

Cached cover bytes carry their own `ETag` sibling and revalidate with
`If-None-Match` *after* the cached image has been served, never before —
skipped while offline, inside a fresh window, without a stored validator, or
while a check of the same key is in flight. Without all four, a grid scroll
becomes one conditional request per visible cover.

## Asking about many files is one request

A client with N downloads must not ask N questions on a timer. `POST
/api/downloads/validators` answers a whole device in one small request and
carries no metadata — the alternative (a full per-book metadata fetch each
tick) is a data and battery cost on the phone and an O(N) load cost on the
server, and it grows with the reader's library.

Both clients sweep — `refresh_stale_flags` in
`frontend/src/offline/downloads/staleness.rs`, `refreshStaleFlags` in
`omnibus-ios/omnibus/Offline/DownloadStaleness.swift`. A client that does not
sweep is not merely slower: iOS saw a replaced file only when the reader
opened that book's own detail page, because the screen listing downloads reads
the library projection, which carries no per-file rows to compare against.

Three things follow, and each has already been got wrong:

- **Ask on a TTL, not on every tick.** A file changing on the server is not
  urgent; a 60-second poll is a regression dressed as freshness.
- **A sweep that asked nothing has not asked.** Only a completed round trip
  may stamp the TTL. Stamping a partial one records work that never
  finished; stamping an *empty* registry — which on iOS hydrates
  asynchronously, so "no downloads" and "not loaded yet" look alike at
  launch — skips the first real sweep for a whole window.
- **Do not smuggle metadata through it.** Writing a fetched `EbookMetadata`
  straight into the cache with `put_json` bypasses the compare/put/notify
  path, so an open page keeps rendering the old fields even though the cache
  now holds new ones. If a refresh needs metadata, it goes through
  `cache::read_through`; if it only needs validators, it uses this endpoint
  and touches no cache at all.

## Out of scope

- How a response carries a validator, the `serve_file` path every
  byte-serving endpoint takes, and why `ETag` and `Vary` must travel
  together — [09a-serving-validated-bytes.md](09a-serving-validated-bytes.md).
- Change feeds / tombstones / a sync cursor — deferred F13 work; no design
  doc has been written for it yet. This is per-resource validation, not a
  cursor.
- What a client may *queue* while offline — see
  [08-offline-writes.md](08-offline-writes.md).
