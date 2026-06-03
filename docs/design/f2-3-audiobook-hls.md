# F2.3 audiobook HLS — multi-file books + uniform stitch-and-stream

## 1. Context

The current basic player (PR #327) treats every audio file as its own book. Real libraries store mp3 audiobooks as **one folder of per-chapter mp3s** — Brandon Sanderson's *Way of Kings* surfaces as 20 separate "books" instead of one. Adjacent issue: m4b vs. mp3 currently use different file routes, and there's no single timeline across a folder of mp3 parts.

This change replaces the per-file model with an **audiobook = folder/file** model and routes every audiobook through a uniform HLS transcode-and-stream pipeline so the player sees one continuous timeline regardless of source shape (single m4b, single mp3, folder of mp3s).

Roadmap: [docs/roadmap/2-3-audiobook-player.md](../roadmap/2-3-audiobook-player.md). Builds on the F2.3 foundation merged in #327 (single-file audiobook indexing + basic player); lands directly on `main`.

**In scope**
- Scanner groups leaf folders of audio files into one audiobook entity.
- New `book_file_parts` rows (one per source file) so multi-part books have an ordered track list at rest.
- ffmpeg-backed HLS pipeline: one master playlist per `(book, profile)`, lazy segment generation, on-disk segment cache with FIFO-by-mtime eviction.
- Single audio profile: `audio64` (AAC-LC 64 kbps mono, 10 s segments).
- `GET /api/audiobooks/{uuid}/playlist.m3u8` + `GET /api/audiobooks/{uuid}/segments/seg-NNNN.ts` replace the current `/file` route.
- hls.js vendored under `frontend/assets/vendor/` and loaded only on `/listen/:uuid`.
- ffmpeg added to `flake.nix` dev shell + the runtime container expected to ship the same binary.

**Not in scope** (deferred to later F2.3 / F5.x increments)
- Chapter atom extraction + chapter list UI.
- Bookmarks (schema is already there from migration 0013).
- Sleep timer / cast / sharing.
- Multiple bitrate profiles (one fixed 64 kbps mono is enough for spoken audio).
- Real-time transcode during initial seek-ahead — first read serializes until the requested segment exists on disk.
- Per-track metadata in the player ("Chapter 14" surfaced from filenames) — comes free with the chapter-list increment.

---

## 2. Data flow

### 2a. Indexing (Phase A stat → Phase B group + tag-read → sync)

```
audiobook library root
        │
        ▼
┌───────────────────────────────────────┐
│  audiobook::stat_audiobook_library    │  walks tree, returns one StatEntry
│  (Phase A — unchanged from PR #327)   │  per audio file
└───────────────────────────────────────┘
        │  Vec<StatEntry>
        ▼
┌───────────────────────────────────────┐
│  audiobook::group_into_books          │  ★ new ★ groups by parent-dir for
│  (Phase A.5 — new in this change)     │  mp3 files; single-file m4b/m4a/mp3
│                                       │  stays a one-entry group
└───────────────────────────────────────┘
        │  Vec<AudiobookGroup>  ─── filename = parent dir path (or single
        ▼                            file's stem); ordered parts inside
┌───────────────────────────────────────┐
│  audiobook::parse_groups              │  ★ new ★ reads ID3 tags from
│  (Phase B — extends parse.rs)         │  every part (lofty), derives:
│                                       │   - title  := album OR dir-name
│                                       │   - author := artist OR parent-dir
│                                       │   - cover  := first part's embedded
│                                       │     artwork
│                                       │   - parts  := sorted by (track,name)
└───────────────────────────────────────┘
        │  Vec<IndexedAudiobook>
        ▼
┌───────────────────────────────────────┐
│  sync::sync_audiobooks                │  ★ new path, mirrors sync_books ★
│  (Phase C — extends sync.rs)          │  writes one books row + one
│                                       │  book_files row (format=AUDIOBOOK)
│                                       │  + N book_file_parts rows
└───────────────────────────────────────┘
```

**Shadow paths**

| Shadow | Behavior |
|---|---|
| **nil/None** — library_path setting unset | Indexer skips audiobook reindex entirely. Already covered by `is_stale` + `last_indexed_at` gate. |
| **empty** — library directory exists but has no audio files | `stat_audiobook_library` returns empty entries; `group_into_books` produces zero groups; `sync_audiobooks` deletes any previously-indexed audiobooks (Removed bucket). |
| **upstream error** — `read_dir` fails on a subdirectory | Existing placeholder-entry pattern: surface as `IndexedBook { error: Some("could not read directory…") }`. Group step skips empty-uuid entries. |
| **upstream timeout** — N/A (filesystem walk is synchronous) | N/A — walks are local; no timeouts. ffmpeg has its own timeout, see §2c. |

### 2b. Manifest fetch (browser → /playlist.m3u8)

```
hls.js  ──GET /api/audiobooks/{uuid}/playlist.m3u8──▶  AuthUser gate
                                                              │
                                                              ▼
                                                ┌─────────────────────────┐
                                                │  audiobook_hls::manifest │
                                                │  reads:                  │
                                                │   - book_file_parts dur. │
                                                │   - total_duration_secs  │
                                                │  builds m3u8 in memory   │
                                                └─────────────────────────┘
                                                              │
                                                              ▼
                                                   200 OK + Content-Type:
                                                   application/vnd.apple.mpegurl
                                                              │
                                                              ▼
                                            hls.js parses, fetches seg-NNNN.ts
```

**Manifest contents** (static; one per book, no per-bitrate variants):

```
#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:10
#EXT-X-PLAYLIST-TYPE:VOD
#EXT-X-MEDIA-SEQUENCE:0
#EXTINF:10.000,
seg-0000.ts
#EXTINF:10.000,
seg-0001.ts
…
#EXTINF:7.532,
seg-5103.ts
#EXT-X-ENDLIST
```

**Shadow paths**

| Shadow | Behavior |
|---|---|
| **nil/None** — uuid not in DB | 404 Not Found. |
| **empty** — book has zero parts (shouldn't happen post-indexer) | 500 Internal Server Error with `tracing::error!` — invariant violation. |
| **upstream error** — DB read fails | 500 via `internal()` (existing pattern). |
| **upstream timeout** — N/A | N/A — manifest is a single read. |

### 2c. Segment fetch (browser → /segments/seg-NNNN.ts) — the hot path

```
hls.js ──GET /api/audiobooks/{uuid}/segments/seg-0042.ts──▶ AuthUser
                                                                  │
                                                                  ▼
                                                ┌─────────────────────────────┐
                                                │ resolve uuid → book_id      │
                                                │ check $DATA/hls/<book>/audio64/
                                                │       seg-0042.ts on disk    │
                                                └─────────────────────────────┘
                                       cache hit         │       cache miss
                                            │            │            │
                                            ▼            │            ▼
                                  ServeFile → 200        │   acquire (book,profile) Mutex
                                  (Range, ETag, etc.)    │            │
                                                         │            ▼
                                                         │   re-check disk (peer raced ahead)
                                                         │            │
                                                         │     still miss
                                                         │            │
                                                         │            ▼
                                                         │  Worker::HlsTranscode {
                                                         │     book_id, profile,
                                                         │     parts: Vec<PathBuf>,
                                                         │  } — runs ffmpeg ONCE for the
                                                         │  whole book, writes all segments
                                                         │  into the cache dir atomically
                                                         │  (one tmp file → rename per seg)
                                                         │            │
                                                         │            ▼
                                                         │  release mutex
                                                         │            │
                                                         │            ▼
                                                         └──▶ ServeFile → 200

After the transcode: subsequent segment requests on the same book hit
the disk cache directly and never enter the transcode path.
```

**Why one-shot per book, not per-segment**: ffmpeg startup cost (~200 ms) dominates per-segment work. Producing all segments in one ffmpeg call writes the full book to disk in one pass. A user opening a 10 h book pays ~30 s of wall-clock CPU once; thereafter every segment is a direct file read.

**While the transcode runs**, the first segment request blocks on the per-`(book, profile)` Mutex. hls.js sees a single slow segment fetch, then subsequent fetches return instantly. Acceptable UX trade vs. the complexity of streaming partial output.

**Shadow paths**

| Shadow | Behavior |
|---|---|
| **nil/None** — uuid not in DB | 404. |
| **empty** — segment index out of range (e.g. seg-9999.ts on a 100-segment book) | 404 (file does not exist after transcode). |
| **upstream error** — ffmpeg exits non-zero | Caller blocked on mutex receives 500. Cache directory is partially-cleaned (see §4: `HlsTranscodeFailed`). |
| **upstream timeout** — ffmpeg hangs | `tokio::time::timeout(30 min, ffmpeg)`. On timeout: kill child, clean partial output, 500. Configurable via `OMNIBUS_HLS_TRANSCODE_TIMEOUT_SECS` (default 1800). |

### 2d. Cache eviction (background, non-request path)

```
On every successful transcode finish:
        │
        ▼
   du -sb $DATA/hls/   (cheap, one stat per book dir)
        │
        ▼
   total > OMNIBUS_HLS_CAP_BYTES?    no → done
        │ yes
        ▼
   list cache dirs by `mtime` ASC, oldest first
        │
        ▼
   rm -rf cache_dir until total ≤ cap
```

Mirrors the existing `thumbs::evict_if_over_cap` pattern. Default cap 5 GiB. Eviction granularity is **the whole book directory**, not individual segments — partial books are useless and re-transcoding from cold is the only repair.

**Shadow paths**

| Shadow | Behavior |
|---|---|
| **nil/None** — cap unset | Use default 5 GiB. |
| **empty** — cache empty | du returns 0; eviction is a no-op. |
| **upstream error** — rm fails (busy file, perms) | `tracing::warn!`; continue with next-oldest. Worst case the cap is exceeded until the next eviction pass succeeds. |
| **upstream timeout** — N/A | N/A — eviction is bounded by cache size. |

---

## 3. Storage shape

### Migration `0014_audiobook_parts.sql`

```sql
-- Multi-part audiobook source files. One row per source audio file under
-- a book_files row. Single-file audiobooks (m4b / single mp3) still get a
-- single part row with ordinal=0 so the read path is uniform.
CREATE TABLE book_file_parts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    book_file_id    INTEGER NOT NULL REFERENCES book_files(id) ON DELETE CASCADE,
    ordinal         INTEGER NOT NULL,
    filename        TEXT    NOT NULL,
    size_bytes      INTEGER NOT NULL,
    mtime_epoch     INTEGER NOT NULL DEFAULT 0,
    duration_seconds REAL   NOT NULL DEFAULT 0,
    UNIQUE(book_file_id, ordinal)
);

CREATE INDEX idx_book_file_parts_lookup ON book_file_parts(book_file_id, ordinal);

-- One-time backfill: every existing audiobook row (M4B / M4A / MP3) gets a
-- single part row pointing at its own filename. Ebook formats (EPUB) are
-- left alone — no part rows.
INSERT INTO book_file_parts (book_file_id, ordinal, filename, size_bytes, mtime_epoch)
SELECT id, 0, filename, size_bytes, mtime_epoch
  FROM book_files
 WHERE format IN ('M4B', 'M4A', 'MP3');
```

**Column-by-column rationale**

| Column | Why |
|---|---|
| `book_file_id` FK with cascade | A book deletion cascades through book_files (already) → now also through book_file_parts. No orphaned rows. |
| `ordinal` | Playlist ordering. Filename-sort is unstable across rip tools; ID3 `track` tag wins, filename is the tiebreaker. |
| `filename` | Same path-relative-to-books.path shape `book_files.filename` already uses. Resolved through `book_file_path` for actual on-disk lookup. |
| `size_bytes`, `mtime_epoch` | Per-part stat so the incremental indexer detects part-file changes (one chapter file replaced) without re-walking. |
| `duration_seconds REAL` | Manifest math: sum across parts = total duration. Required for `EXT-X-TARGETDURATION` and segment count. Persisted because lofty's per-file probe is the slowest part of indexing (sub-second per file but multiplied by hundreds of files). |
| `UNIQUE(book_file_id, ordinal)` | Constraint matches the read pattern: every part is uniquely identified by `(book, ordinal)`. |

**Migration reversibility**: forward-only. `DROP TABLE book_file_parts` is trivial; the data it carries is derivable from the on-disk files, so a rollback simply discards it.

### Hot queries

1. **Manifest build** (per request):
   ```sql
   SELECT bf.id,
          (SELECT COALESCE(SUM(duration_seconds), 0)
             FROM book_file_parts WHERE book_file_id = bf.id) AS total_secs
     FROM book_files bf
     JOIN books b ON b.id = bf.book_id
    WHERE b.uuid = ? AND bf.format IN ('M4B', 'M4A', 'MP3')
    LIMIT 1;
   ```
2. **Part resolution for ffmpeg invocation** (once per transcode):
   ```sql
   SELECT p.filename
     FROM book_file_parts p
    WHERE p.book_file_id = ?
    ORDER BY p.ordinal;
   ```
3. **Indexer staleness** (every reindex):
   ```sql
   SELECT id, mtime_epoch, size_bytes
     FROM book_file_parts WHERE book_file_id IN (…);
   ```

All three are O(parts) which is bounded by leaf-folder size (a few hundred at most). Index on `(book_file_id, ordinal)` covers them.

### Filesystem layout for HLS segments

```
$OMNIBUS_DATA_DIR/hls/<book_id>/audio64/
   ├── playlist.m3u8        (optional — only if we cache built manifests; default no)
   ├── seg-0000.ts
   ├── seg-0001.ts
   …
   └── seg-NNNN.ts
```

`<book_id>` uses the integer PK, not the uuid — segment files are an internal cache, callers never see this path. Eviction targets the `<book_id>` directory atomically (rm -rf).

`$OMNIBUS_DATA_DIR` defaults to `./data` and is configurable. The cap lives at `OMNIBUS_HLS_CAP_BYTES` (default 5 GiB), parallel to `OMNIBUS_THUMBS_CAP_BYTES`.

---

## 4. Failure modes

| Failure | Cause | Detection | Recovery | User sees |
|---|---|---|---|---|
| `AudiobookGroupEmpty` | Indexer hands a folder with zero readable audio files to `parse_groups` | `parse_groups` returns `Err(AudiobookError::Empty)`; surfaces as an `IndexedBook { error: Some(_) }` row | Skipped at sync time (won't write a books row); next reindex retries | No effect — the book simply isn't in the library |
| `LoftyTagRead` | Source file is corrupt or unsupported codec inside an mp3/m4b container | Per-file `lofty::read_from_path` error | Per-part error captured; the book is still indexed using filename fallbacks for title/author and the part is skipped from playback | Book appears in library, playback may be missing one chapter — log line carries the path |
| `MissingDuration` | lofty returns 0 / inf for `properties().duration()` (rare on malformed files) | `duration_seconds <= 0` after parse | Skip the part from playback (would break manifest math); log a warn | Same as above — missing chapter |
| `HlsTranscodeFailed` | ffmpeg exits non-zero (codec error, bad input, OOM) | `Command::status().success() == false`; stderr captured | Mutex released, partial output cleaned (`rm -rf <book_id>/<profile>`), 500 returned to current waiter. `tracing::error!` with stderr. Next request retries from cold. | Player surfaces "This book couldn't be loaded." (existing reader-style overlay) |
| `HlsTranscodeTimeout` | ffmpeg hangs past `OMNIBUS_HLS_TRANSCODE_TIMEOUT_SECS` (1800 s default) | `tokio::time::timeout` future | Kill child process (`Child::kill().await`), clean partial output, 500. Mutex released. | Same as above |
| `HlsCacheEvictionFailed` | rm on a busy cache dir fails | Non-fatal `tracing::warn!` per directory | Skip to next-oldest. Cap may be exceeded temporarily; cleared next eviction. | No effect — internal cache management |
| `SegmentMissingPostTranscode` | Race: request arrives for seg-NNNN.ts after transcode finished but segment file is absent (ffmpeg produced fewer segments than the manifest claims) | `ServeFile` 404 surfaces; handler maps to 500 because manifest claimed it exists | `tracing::error!` with book + segment index. User receives 500. Next book open re-transcodes from cold. | "This book couldn't be loaded." |
| `Hls.jsLoadFailed` | hls.js asset 404 or parse error in the browser | `Hls.Events.ERROR` callback | Fall back to native `<audio src=…m3u8>` (Safari plays HLS natively; other browsers will fail with an `error` event the existing status overlay handles) | "This book couldn't be loaded." on non-Safari; Safari plays normally |
| `PartFilenameNotOnDisk` | Source mp3 deleted between indexer pass and a transcode request | ffmpeg exits non-zero with `No such file or directory` | Surfaces as `HlsTranscodeFailed`; recovery is the same. Next reindex sweeps the missing file out of `book_file_parts`. | "This book couldn't be loaded." until next reindex |
| `MutexPoisoning` | Panic inside the transcode worker holding the per-`(book, profile)` Mutex | `tokio::sync::Mutex` doesn't poison (unlike std); but a panic mid-ffmpeg would still leak the lock-guard if not unwound | Worker handler `spawn_blocking` boundary catches the panic, logs it, releases the guard via drop. | Same as `HlsTranscodeFailed` |
| `DiskFull` | `<data_dir>/hls` writes fail with `ENOSPC` | ffmpeg exits with `No space left on device` | `HlsTranscodeFailed` path; eviction kicks aggressively (oldest 50% of cache). | "This book couldn't be loaded." until eviction frees space |

---

## 5. Rollback plan

**Schema rollback**: forward-only migration. `DROP TABLE book_file_parts` is non-destructive — its contents are derived from on-disk files and re-generated on next reindex. Reverting the binary to the F2.3-foundation (PR #327) commit while leaving `book_file_parts` rows in the DB is harmless: the previous code never touches the new table.

**Code rollback**: `git revert` of this commit, plus a forced reindex (touch `last_indexed_at` to 0 or delete library settings row and re-save). The old per-file model re-indexes from scratch on next worker tick.

**Cache rollback**: `rm -rf $OMNIBUS_DATA_DIR/hls/`. Always safe — pure cache.

**ffmpeg dependency rollback**: `flake.nix` revert removes the package from the dev shell. Production: revert the deploy. Without ffmpeg, the manifest route would 500 on transcode trigger; the foundation `/file` route is removed by this PR, so a partial rollback (binary new, ffmpeg gone) breaks playback. **Either revert both or neither.** Captured here so the operator doesn't try a partial revert.

**Feature flag**: not introduced. Justification: the new behaviour is the only meaningful behaviour once the schema lands. A flag would multiply test surface for negligible isolation benefit. Killswitch is the same as the rollback path above.

---

## 6. Observability

**Logs**

| Event | Level | Fields |
|---|---|---|
| Audiobook indexed | `INFO` | `book_id`, `uuid`, `part_count`, `total_duration_secs`, `title` |
| Audiobook indexing error (whole group) | `WARN` | `dir`, `error` |
| Part parse error | `WARN` | `file`, `error` (one log per part — bounded by library size) |
| HLS transcode start | `INFO` | `book_id`, `profile`, `part_count`, `expected_segment_count`, `total_duration_secs` |
| HLS transcode finish (success) | `INFO` | `book_id`, `profile`, `elapsed_secs`, `bytes_written` |
| HLS transcode failure | `ERROR` | `book_id`, `profile`, `elapsed_secs`, `ffmpeg_stderr_tail` (last 4 KiB) |
| HLS transcode timeout | `ERROR` | `book_id`, `profile`, `timeout_secs` |
| Cache eviction | `INFO` | `freed_bytes`, `evicted_book_ids` (count, not list, if > 10) |
| Cache eviction failure | `WARN` | `book_id`, `error` |

**Metrics** (Prometheus-style — wire when omnibus grows a `/metrics` endpoint; the names are reserved here so handlers emit them once the endpoint exists):

| Metric | Type | Labels |
|---|---|---|
| `omnibus_audiobook_indexed_total` | counter | — |
| `omnibus_hls_transcode_total` | counter | `outcome=success\|failure\|timeout` |
| `omnibus_hls_transcode_duration_seconds` | histogram | — |
| `omnibus_hls_cache_bytes` | gauge | — |
| `omnibus_hls_cache_evictions_total` | counter | — |

**Alerts** (deferred along with the `/metrics` endpoint, but worth naming so they're not invented from scratch later):
- `omnibus_hls_transcode_total{outcome="failure"}` > 5/hour → page owner.
- `omnibus_hls_cache_bytes` > 90% of cap for > 1 h → notify (eviction is falling behind).

**Dashboards**: a new "Audiobook playback" panel on the existing omnibus dashboard once the metrics endpoint lands. Tracked under F5.2 (observability initiative). No dashboard work in this PR — only structured logs.

---

## 7. Open questions

### Resolved

- **HLS over Range-served mp3** → uniform pipeline; user explicitly chose this over keeping single-file mp3 / m4b on direct-play. Simpler internal code path, slightly higher CPU/disk cost per book.
- **ffmpeg in Nix** → added to `flake.nix`; production deploys must ship ffmpeg in the runtime image.
- **Transcode strategy** → one ffmpeg per `(book, profile)` produces the entire HLS output in one pass. Per-segment ffmpeg invocations rejected (200 ms startup × thousands of segments).
- **Audio profile** → single fixed profile `audio64` (AAC-LC 64 kbps mono). Spoken audio doesn't need stereo or higher bitrate. Multi-profile is a future ticket if/when music audiobooks land.
- **Segment duration** → 10 s. Standard Apple/HLS recommendation, balances seek granularity (10 s is the maximum jump on a "next segment" decision) against transcode/storage overhead.
- **Part ordering** → ID3 `track` tag ascending, filename ascending as tiebreaker. Filename-only ordering is unstable across rip tools.
- **Title source** → ID3 `album` tag first, parent directory name fallback. Brandon Sanderson's library demonstrates that filenames are noisy (`MISTBORN05P01.mp3`) but album tags are clean (`Shadows of Self`).
- **Cache key includes profile** → yes, `audio64`. Future second profile won't collide.
- **Schema shape** → `book_file_parts` keyed on `book_file_id`, not `books.id`. Mirrors the existing book_files split (F0.1) so future per-format part lists (e.g. CBR comic chapters) slot in.
- **Cover artwork** → first part's embedded artwork. Unchanged from PR #327 except now sourced from `parts[0]` instead of "the only file".
- **Existing /file route** → removed (introduced by the merged #327). The new manifest + segment routes replace it; the listen page swaps over in the same change so there's no stable middle state.
- **Concurrent transcode cap** → separate `Semaphore(max(1, num_cpus / 2))` for HLS transcodes so the scan semaphore (1 permit) isn't blocked by a long-running transcode. New `WorkerConfig::hls_concurrency` knob, default = `max(1, num_cpus / 2)`.
- **Reindex vs. in-flight transcode** → accept the race. ffmpeg errors out when a source file is deleted mid-transcode; partial cache cleans per `HlsTranscodeFailed`. Deleted books aren't requested again anyway.
- **First-segment latency UX** → add `GET /api/audiobooks/{uuid}/status` returning `{ ready: bool, progress: f32 }`. Listen page polls every 1 s while `!ready` and shows a "Preparing your book…" overlay with the progress bar. Transcode worker writes `<book_id>/<profile>/.progress` (atomic 0.0–1.0 value) every N seconds; status endpoint reads it.

### Unresolved

- **Manifest caching** → re-build per request (cheap: one SQL + string format) vs. write `playlist.m3u8` to disk alongside segments. **Defer to bench.** Default: build per request.

---

## 8. Test plan

### Happy path
- **Indexer**: a fixture library containing one m4b file + one mp3 folder (3 mp3s with tracks 1/2/3 and ID3 tags) → exactly 2 `books` rows; the mp3 folder's row has 3 `book_file_parts` rows in `(0, 1, 2)` order; the m4b row has 1 part row at ordinal 0.
- **Manifest**: `GET /api/audiobooks/{uuid}/playlist.m3u8` returns a valid m3u8 with `EXT-X-VERSION:3`, `EXT-X-TARGETDURATION:10`, `EXT-X-ENDLIST`, and segment count = `ceil(total_secs / 10)`.
- **Segment fetch**: `GET /api/audiobooks/{uuid}/segments/seg-0000.ts` returns 200 + `application/vnd.apple.mpegurl` is wrong — actual `video/MP2T`. Body is a valid MPEG-TS frame (assert the sync byte 0x47 at offset 0 + every 188 bytes).
- **End-to-end**: drive the listen page via the `ui-validate` skill; assert hls.js plays through at least 3 segments without error; assert position persistence still works across reload.

### Failure modes (one negative test each)

| Failure | Test |
|---|---|
| `AudiobookGroupEmpty` | Library with an empty subfolder → 0 books indexed, no error row. |
| `LoftyTagRead` | A `garbage.mp3` in a folder → other parts still index, the bad part is omitted from `book_file_parts`. |
| `MissingDuration` | Synthesised mp3 with corrupt duration → part omitted from playback math. |
| `HlsTranscodeFailed` | Mock ffmpeg returning exit code 1 (via `OMNIBUS_FFMPEG_PATH` env override pointing at `/bin/false`) → 500 on segment fetch, cache dir cleaned. |
| `HlsTranscodeTimeout` | Mock ffmpeg `sleep 5` + `OMNIBUS_HLS_TRANSCODE_TIMEOUT_SECS=1` → 500, child reaped, cache dir cleaned. |
| `HlsCacheEvictionFailed` | Pre-populate cache with a file under a chmod-555 dir → eviction logs WARN and moves on; subsequent eviction over a writeable dir succeeds. |
| `SegmentMissingPostTranscode` | Mock ffmpeg that produces fewer segments than the manifest claims → manifest path serves 200, missing segment 500s. |
| `Hls.jsLoadFailed` | (Manual / Playwright) — break the vendored asset URL; non-Safari shows error overlay. |
| `PartFilenameNotOnDisk` | After indexing, delete one source mp3 → transcode 500s; reindex sweeps the part row. |
| `DiskFull` | Mount the HLS cache dir on a tiny tmpfs (1 MB); trigger transcode → 500, no panic. |

### Integration
- A single E2E test that walks the full §2a → §2c flow against a checked-in fixture library (1 m4b + 1 three-part mp3 folder, both fitting in ~2 MB). Assertion: index → manifest → segments → playback all succeed.

### Not tested
- **hls.js itself** — vendored unchanged; no value re-testing the library.
- **ffmpeg invocation flags on every platform** — we test against the Nix-pinned ffmpeg; downstream operators run their own ffmpeg at their own risk. Documented in CLAUDE.md.
- **Concurrent transcode behaviour under load** — the Mutex covers correctness; perf is out of scope for the basic player.

---

[← Back to roadmap initiative](../roadmap/2-3-audiobook-player.md)
