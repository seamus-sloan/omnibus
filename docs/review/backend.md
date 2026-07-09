## Backend Endpoints & Transport Review

### Overall Technical Score: 58/100

> API design 14/25 — two hand-duplicated transport surfaces with no shared contract, no pagination on heavy reads, and uneven verb/status modeling. Auth & security 11/20 — two permission columns never enforced, RPC error-string leaks, split admin gating, and a /api/-prefix assumption that leaves /opds/* open. Consistency 11/20 — the REST and RPC copies have already drifted (non-atomic batch, manual vs extractor admin checks, generic-500 vs raw-error). Performance 11/20 — a blocking HLS segment handler that 408s under the 30s timeout, double session validation per request, 1Hz polling, full-file EPUB buffering, and a timeout-less mobile client. Future-fit 11/15 — sound REST/JSON + SSE direction (gRPC correctly rejected) but no API versioning and structural duplication that taxes every roadmap feature.

---

### High Priority — Two transport surfaces hand-duplicate every operation

Every web operation exists twice on the server — a Dioxus server function in `frontend/src/rpc.rs` (`/api/rpc/*`) and a hand-written axum handler in `server/src/backend/*` (`/api/*`) — and twice again on the client (web rpc wrapper vs mobile reqwest wrapper in `frontend/src/data/*`). The two server impls call the same `omnibus_db` functions but independently re-derive auth, validation, error mapping, and response shaping, with no shared contract or compiler check that they agree.

This is the root cause of three separate drift bugs below (non-atomic web session batch, manual-vs-extractor admin checks, RPC error leaks) — each is the same logical operation fixed on one copy and silently skipped on the other. The duplication is structural: the `AuthUser`/`AdminUser` extractors are copy-pasted (`frontend/src/rpc.rs:64-145` mirroring `server/src/auth/extractor.rs:69-129`), and the `rpc.rs` copy has already drifted — its `AuthUser` carries only `id`/`is_admin`/`can_edit` (`rpc.rs:74-80`) and drops `can_upload`/`can_download`/`session_kind` that the real extractor carries (`extractor.rs:23-27`).

With OPDS (`/opds/*`, F4.2), Kobo/Kindle sync, uploads, ratings, journaling, shelves, and libraries all still ahead, every new feature pays this tax. The comment at `rpc.rs:50-62` justifies why the *extractor* is duplicated (frontend can't depend on the server crate; dioxus re-exports axum) but not why the *business logic* is.

Fix direction: collapse to one surface. Either have the web client call the same `/api/*` REST routes mobile uses (the gate already covers `/api/*` uniformly — `gate.rs:42`) and delete `/api/rpc/*`; or extract the per-op logic (validation + auth gate + db call sequence) into a shared service layer in `db/` or a new module that both routers call as thin shims, so a fix to one path can't skip the other.

---

### High Priority — Web session-batch ingest is non-atomic

The mobile REST `POST /api/progress/sessions` was deliberately made atomic (commit f72fa0f): `server/src/backend/progress.rs:84-99` opens one transaction, calls `record_session_tx` per report, and commits once, so a mid-batch DB error rolls back the whole batch and the client can retry without double-counting. The web RPC equivalent `rpc_record_sessions` (`frontend/src/rpc.rs:561-575`) was not updated to match: it loops over reports calling `db::progress::record_session`, which opens and commits its own transaction per call (`db/src/progress.rs:199-208`).

So a failure on report N leaves reports `0..N-1` committed and returns an error to the web client with no recorded count, and a client retry re-inserts the already-committed reports — exactly the double-count the REST fix prevents. The db layer documents the hazard explicitly: `record_session`'s rustdoc says "For batch inserts, prefer `record_session_tx` inside a caller-managed transaction so the entire batch rolls back atomically" (`db/src/progress.rs:196-198`) — advice the web path ignores.

Session reports feed F2.1 stats / year-in-review aggregates, so double-counted reading/listening time corrupts user-visible numbers. This is the clearest single illustration of the dual-surface maintenance tax. Fix: rewrite `rpc_record_sessions` to mirror the REST handler — one `pool.begin()`, `record_session_tx` in the loop, single commit, returning the inserted count — or route both surfaces through one shared transactional function.

---

### High Priority — HLS segment handler blocks behind request timeout

`get_audiobook_segment` (`server/src/backend/audiobooks.rs:275-330`) handles a cache miss by posting `Task::HlsTranscode` and then doing `let _ = state.worker.await_completion(task_id).await;` (line 312) — it blocks the request task until the *entire* transcode of the audiobook finishes, then re-checks for the file. The transcode timeout defaults to 1800s (`OMNIBUS_HLS_TRANSCODE_TIMEOUT_SECS`, `db/src/hls.rs:501`), but every request is wrapped in a 30s `TimeoutLayer` in both `server/src/main.rs:218-221` and `server/src/backend.rs:161-164`.

So for any HLS-fallback book whose first requested segment isn't already on disk, the segment request returns 408 after 30s while the transcode keeps running for up to half an hour. hls.js sees a 408 on the first segment and surfaces a playback failure even though the transcode is progressing fine. The non-blocking design already exists: `/status` returns `{state: preparing|ready|failed}` and the client (`frontend/src/pages/listen/bootstrap.rs:532-576`) polls it at 1Hz, only attaching hls.js once `state==ready` — so the segment handler blocking on the same transcode is both redundant and harmful, and it pins a tokio task plus a connection for up to 30s per concurrent cold segment.

Fix: make the segment endpoint non-blocking — if the segment is absent and not failed, kick the transcode fire-and-forget (as `/status` already does) and return 503/425/404 immediately, letting hls.js retry after the `/status` poll flips to ready. The `has_failed` short-circuit at line 302 already returns 503; the `await_completion` branch should never run.

---

### High Priority — Download/upload permission columns never enforced

F0.3 added `is_admin` / `can_upload` / `can_edit` / `can_download` permission columns and F0.7 was to wire them onto every route, but two of the four axes are decorative.

`can_download` is loaded into `AuthUser` (`server/src/auth/extractor.rs:25,106`) and returned to clients via `UserSummary` / `/api/auth/me` (`extractor.rs:39`), but no handler ever consults it — a repo-wide search finds it only in struct definitions, the `UserSummary` projection, and test seeds, never in an authorization branch. The raw-content endpoints — `get_ebook_file` (`server/src/backend/ebooks.rs:56`, streams the EPUB) and the audiobook part/segment/playlist streams (`audiobooks.rs:171,275,51`) — gate on bare `_user: AuthUser` with no `can_download` check, so a user with `can_download=false` downloads every file. The `rpc.rs` `AuthUser` doesn't even carry the field (`rpc.rs:74-80`), so the web surface couldn't enforce it even if a handler wanted to.

Symmetrically, the upload/edit endpoints never consult `can_upload`: `post_ebook_cover` (`overrides.rs:110-115`) gates on `can_edit`, and the two author-photo PUTs the route comment frames as "binary-upload endpoints" (`backend.rs:48-58`) gate on `AdminUser` (admin-only; `author_photos.rs:65,249`) — so the column an admin would toggle to revoke upload rights does nothing.

This blocks the multi-user roadmap (F3.1 shelves, F5.4 user management, F4.2 OPDS download). Fix: decide the semantics (likely `can_download` gates file/stream endpoints and OPDS download; `can_upload` gates cover/photo and future file uploads), add checks mirroring the existing `if !user.is_admin && !user.can_edit` pattern, add the missing fields to `rpc.rs` `AuthUser`, and add anon-401 + forbidden-403 sibling tests. If the product decision is that all authed users may download, delete the columns rather than advertising controls that do nothing.

---

### Resolved — RPC server functions leaked raw DB errors (fixed in #761)

The REST surface is disciplined: every backend handler funnels unexpected failures through `internal()` (`server/src/backend.rs:67-77`), which logs the full error via `tracing::error!` and returns a fixed "internal server error" body, so sqlx/driver internals never reach the wire. The `/api/rpc/*` server functions used to do the opposite; `rpc.rs` has since been split into `frontend/src/rpc/*.rs` submodules.

Explicit leaks (stringified `Sqlx`/catch-all error arms) and implicit leaks (bare `?` on db calls whose `Display` crossed the wire via the `ServerFnError` `From` conversion, landing in `DataError::Other(e.to_string())` at `frontend/src/data.rs:513`) are both **fixed**: every opaque-failure call site across `frontend/src/rpc/*.rs` now routes through `internal_rpc_error` (`frontend/src/rpc/mod.rs`), which logs the real error via `tracing::error!` and returns a fixed opaque `ServerFnError`, mirroring `server/src/backend.rs::internal`. Mixed-variant error enums (`MergeError`, `ShelfError`) are matched explicitly so their typed/validation variants (`SameBook`, `NameTaken`, `BookNotFound`, `InvalidRule`, …) still surface their specific message; only the opaque `Sqlx`/transport/IO variants are genericized. The equivalent REST-side leak in `server/src/backend/kindle.rs::post_smtp_test` (a raw `KindleError::to_string()` on `BAD_GATEWAY`) was fixed the same way, mirroring that file's own `internal()` helper.

---

### Medium Priority — Inconsistent admin gating: extractor vs in-body

Admin authorization is expressed two ways across the RPC surface with different wire behavior. Most admin RPCs use the `AdminUser` extractor (`rpc_get_settings`, `rpc_merge_books`, `rpc_delete_author`, `rpc_refetch_author_photos`, etc. — `rpc.rs:147,152,248,259,269,372,381,527`), whose `FromRequestParts` rejects non-admins with a clean HTTP 403 before the body runs (`rpc.rs:137-143`).

But two admin RPCs instead take a plain `AuthUser` and hand-roll `if !user.is_admin { return Err(ServerFnError::new("forbidden: admin required")) }`: `rpc_scan_author_photo` (`rpc.rs:353-357`) and `rpc_set_author_photo_url` (`rpc.rs:414-418`). A `ServerFnError::new(...)` is not a 403 — it serializes as a generic Dioxus server-function error (a 500-class response carrying the string in the body), and the web client's `note_server_fn_err` only special-cases code 401 (`frontend/src/data.rs:507`), so a "forbidden" from these two routes falls through to `DataError::Other`, indistinguishable from a real server fault.

This both muddies the client contract and makes the requirement invisible to the route table — the in-body check is one missing line away from an open admin endpoint, whereas the extractor makes the requirement part of the signature and type-enforced. Fix: convert both routes to the `AdminUser` extractor like their siblings, deleting the inline `is_admin` checks.

---

### Medium Priority — Live-status surfaces use 1Hz polling

Three live-state surfaces are fixed-interval client polls. (1) `rpc_worker_status` is polled at 1Hz continuously by `WorkerStatusIndicator` for as long as the indicator is mounted (`frontend/src/components/worker_status.rs:18,41-47`), re-running the server-fn session validation and serializing a full snapshot every second whether or not anything changed. The handler is even forced to be a POST purely because the Dioxus `#[get]` variant 404s with a `WorkerExt` extractor (documented at `rpc.rs:179-195`), and drags an unused `PoolExt` along — a latent maintenance trap that disappears under SSE. (2) `GET /api/audiobooks/{uuid}/status` is polled every 1s during HLS preparation (`bootstrap.rs:575`), and each poll also fires a worker `HlsTranscode` post as a side effect when `!ready && progress<0.05 && !failed` (`audiobooks.rs:240-247`) — a side-effecting GET that would misbehave behind a caching proxy or prefetcher. (3) Mobile gets nothing: `data/books.rs:200-207` is a no-op stub returning an empty snapshot.

The roadmap multiplies this: F5.2 wants a background-task dashboard, F2.1 wants multi-device progress reconcile, F2.3 transcode progress. The worker already holds the snapshot in memory and uses `tokio::sync::watch` internally (`db/src/worker/queue.rs:13,87`; `db/src/worker/progress.rs:22`), so a single SSE endpoint (`GET /api/events`, `text/event-stream`) fed by a watch/broadcast is cheap and additive — it pushes transcode-ready the instant ffmpeg finishes instead of up to 1s late, retires the 1Hz busy-poll, and gives both clients one mechanism. SSE (not websockets) fits: data is unidirectional server→client, rides plain HTTP/GET so it passes `origin_check` (GET is csrf-exempt, `csrf.rs:43`) and `require_auth` unchanged, auto-reconnects, and needs no new dependency. Keep polling as a mobile fallback until it adopts SSE. Also move the transcode kick out of the status GET into an explicit `POST /prepare`.

---

### Medium Priority — Heavy read paths return whole library

The primary list/discovery endpoints return the entire result set in one JSON blob with only a hard server-side cap (`db::MAX_BOOKS_RETURNED`) as a backstop — no cursor/offset pagination on the wire. `rpc_get_ebooks` / `get_ebooks` return the full combined ebook+audiobook library (`rpc.rs:206-230`, `ebooks.rs:25-40`). `list_authors` and `list_series` return every author/series across both libraries with no cap at all (`server/src/backend/authors.rs:55-67`, `frontend/src/rpc.rs:455-463,466-473`).

The REST ebook path at least surfaces truncation via `X-Total-Count`/`X-Total-Cap` headers (`backend.rs:384-406`), but the RPC side admits it can't even do that — the code comment flags "Dioxus server functions don't expose response headers, so this path can't currently surface a truncated hint... Cursor pagination is the next step" (`rpc.rs:216-223`) — so the web client silently gets a truncated set with no signal.

Every landing-page load ships the whole library; a large install pays full serialization + transfer on each navigation. As F1.x browse/discovery and F3.1 shelves land this becomes a latency and memory problem on both ends. Fix: add cursor pagination (keyset on a stable sort key) to the list endpoints and thread next-cursor/total through the JSON body — the RPC variant controls the body shape even without headers, exactly as `rpc_search` already threads `total` via `EbookLibrary::total` (`rpc.rs:304-310`). The authors/series indexes need the same treatment plus a cap.

---

### Medium Priority — No API versioning on mobile REST contract

The mobile-facing `/api/*` surface has no version prefix or version negotiation — a repo-wide grep for `/api/v1`, `X-API-Version`, `accept-version` returns nothing. The wire DTOs in `shared/` are shared by reference with the web build, but the mobile app is a separately-built, independently-distributed native binary that talks JSON over reqwest and persists a 90-day bearer token, so an installed app can be many releases behind the server.

The codebase already shows the strain: `with_pagination_headers` (`server/src/backend.rs:384-406`) keeps the `EbookLibrary` JSON shape byte-identical "so older mobile clients keep parsing," and `StatusResponse` keeps legacy `ready`/`progress` fields "for compatibility with any client that pre-dates the `state` field" (`server/src/backend/audiobooks.rs:348-358`). These are ad-hoc per-field compatibility hacks substituting for a real versioning strategy. `client_version` is even captured at login but never used to gate responses.

As the roadmap adds Kobo/Kindle sync, OPDS (F4.2, its own `/opds/*` surface), and uploads, breaking changes become inevitable with no mechanism to introduce one without silently breaking old installs. Fix direction: introduce an explicit version segment (`/api/v1/*`) now while the client population is small, or adopt a documented compatibility policy plus a version handshake on login, and pin the DTO contract per version.

---

### Medium Priority — EPUB serve buffers whole file, no Range

`get_ebook_file` reads the entire EPUB into memory with `tokio::fs::read(&path)` and returns the bytes in one shot (`server/src/backend/ebooks.rs:80-90`), with no `Accept-Ranges`/`Range` support and no streaming. The audiobook part handler right next to it correctly uses `ServeFile` (tower-http), which gives Range, conditional requests, and streamed bodies for free (`audiobooks.rs:199-212`).

The inconsistency matters for the roadmap: F2.2 epub reader and F6.2 mobile epub reader will want byte-range reads (readers seek within the zip), and F6.1 offline download streams the active format to disk. Buffering a multi-MB (sometimes 50MB+) EPUB fully into a `Vec` on every open allocates the whole file per concurrent reader and blocks resumable downloads. The global `DefaultBodyLimit` caps requests, not responses, so nothing bounds this allocation.

Fix: serve the EPUB via `ServeFile` / `tower_http::services::fs` like the audio parts do, so Range + streaming + content-length come from one well-tested path. The cover/thumb/author-photo handlers return whole buffers from DB blobs — those are small and acceptable; the EPUB file path is the one that should stream.

---

### Medium Priority — Mobile reqwest client has no timeouts

The shared mobile HTTP client is built with `reqwest::Client::new()` (`frontend/src/data.rs:393-396`), which sets no total-request timeout and no connect timeout. The server protects itself with a 30s `TimeoutLayer`, but a mobile client hitting a hung TLS handshake, a stalled body, or a server that accepted the connection but never responds will wait indefinitely on every data call (progress sync, library load, cover fetch).

On mobile this is the difference between a spinner that recovers and one that hangs until force-quit — and F6.1 explicitly leans on this client for offline-queue drain and progress sync on reconnect, where flaky networks are the norm. The code comment at `data.rs:385-392` already reasons about connection reuse, so the client is the right single place to add `.timeout(Duration::from_secs(30))` and `.connect_timeout(...)`.

`DataError::Network` already exists to surface the timeout, so no API change is needed — just build via `reqwest::Client::builder().timeout(...).connect_timeout(...).build()`.

---

### Medium Priority — Every protected request validates the session twice

Each protected `/api/*` request validates the session against the DB twice. The top-level `require_auth` middleware (`server/src/auth/gate.rs:49-53`) calls `lookup_session` to gate the boundary, and then the per-handler `AuthUser`/`AdminUser` extractor independently calls `validate_session` again (`server/src/auth/extractor.rs:99`) to produce the typed user.

That is two SQLite round-trips (token-hash + session lookup, plus the user-row join in the extractor) on the hot path for every authed call — including the 1Hz worker-status poll and the 1Hz HLS status poll, which together sustain ~2 req/s while a scan or transcode runs. The gate's own doc acknowledges the redundancy: it "does not set per-user request extensions: handlers that need `AuthUser` should still declare it" (`gate.rs:22-24`).

Cleaner: have `require_auth` resolve the user once and stash it in request extensions, and have the extractor read from extensions (falling back to validation only when absent), so the boundary check and the typed view share a single DB hit. Small, but compounds with the polling loops; converting those to SSE also shrinks the blast radius.

---

### Medium Priority — Remote-fetch errors return 500 not 502/400

The author-photo-by-URL endpoints fetch an admin-supplied remote URL server-side. When the remote host fails (connection refused, TLS error, read timeout — a reqwest error surfaced as `FetchRemoteImageError::Http(e)`), both handlers map it to a 500.

In the REST handler `put_author_photo_url`, `FetchRemoteImageError::Http(e)` goes to `internal("fetch remote image", e)` → 500 (`server/src/backend/author_photos.rs:275-277`), while all the *validation* variants (bad scheme, blocked SSRF target, non-image content-type, too-large) are correctly mapped to 400 at lines 278-283. In the RPC `rpc_set_author_photo_url` the whole `fetch_remote_image` result is `.map_err(|e| ServerFnError::new(e.to_string()))` (`frontend/src/rpc.rs:430-432`), which returns a server-error-class response and leaks the raw reqwest `Display` (overlaps the error-leak finding).

A failure to reach an admin-typed external URL is not a fault of this server — it is an upstream/gateway condition (502/504) or bad input (400), and the UI should be able to tell "your URL is unreachable" apart from "the server is broken." Fix: map the `Http` transport variant to 502 (or 400) in both handlers, and stop echoing the raw reqwest `Display`.

---

### Medium Priority — Keep REST/JSON plus SSE; reject gRPC

Addressing the explicit rpc-vs-gRPC question with a verdict: do not adopt gRPC. (1) Browser support — gRPC needs grpc-web plus a proxy (Envoy) because browsers can't speak raw HTTP/2 framing from `fetch`; the web client is the primary consumer and already uses Dioxus server functions over plain HTTP, so adding an Envoy hop to a single-instance self-hosted app is pure operational tax. (2) Both real clients are Rust (browser WASM + native mobile shell) already sharing the `omnibus_shared` serde DTOs (`frontend/src/rpc.rs:18-24`) — gRPC's headline win (polyglot codegen-typed contracts) is moot when both ends import the same crate.

(3) The two streaming-shaped surfaces (worker status, HLS status) are unidirectional server→client and are better served by SSE over the existing HTTP stack (passes `origin_check`/`require_auth` unchanged; GET is csrf-exempt, `csrf.rs:43`) with zero new infra. (4) Range-served binary endpoints (audiobook parts, ebook file, covers) are exactly what HTTP does well and gRPC does poorly.

The real pain is maintaining two JSON surfaces by hand (see the dual-transport finding); gRPC would add a third encoding without removing either. Recommendation: keep JSON/HTTP + serde DTOs, invest the effort in collapsing the REST/RPC duplication that is already causing drift bugs, and add one SSE event stream for live status.

---

### Low Priority — REST verb/status inconsistencies across siblings

Resource/verb modeling is uneven across the REST surface in ways that will confuse mobile and future OPDS/third-party consumers. (1) Overrides delete is modeled inconsistently between surfaces: REST uses `DELETE /api/ebooks/{uuid}/overrides` (`server/src/backend.rs:206-208`) but the RPC mirror is `POST /api/rpc/ebook/overrides/delete` (`frontend/src/rpc.rs:498`) — the web path can't use DELETE because Dioxus can't carry a body on a read-shaped verb, so it degrades. (2) `post_progress` is an upsert that mutates the canonical `(user,book,format)` row but is POST, not PUT/PATCH, so it isn't idempotent at the HTTP layer despite being last-write-wins idempotent in semantics (`server/src/backend/progress.rs:32`). (3) `post_reindex` returns 409 CONFLICT when no library path is configured (`server/src/backend/settings.rs:73-78`) — 409 implies a state conflict, but "no library configured" is closer to 412 Precondition Failed or 422. (4) `get_thumb` returns 202 ACCEPTED for a coverless book (`server/src/backend/covers.rs:124`) from a GET, conflating "resource being generated" with a normal read.

None of these are bugs, but they make the contract harder to document. F4.2 OPDS explicitly aims for client-compatible REST semantics, so a sloppy verb/status baseline propagates. Fix: where Dioxus constraints force POST-for-read on the web side, keep the REST side correct (DELETE/PUT) rather than mirroring the degraded verb, and document the status-code conventions to align the outliers.

---

### Low Priority — Audiobook/EPUB serve lack path-traversal guard

`get_audiobook_part` builds the served path as `Path::new(&resolved.library_path).join(&part.filename)` and hands it to tower_http `ServeFile` with no normalization or containment check (`server/src/backend/audiobooks.rs:199-205`). `ServeFile` does not reject `..` components, so if `part.filename` ever contained `../` the response could escape the library root.

Today `part.filename` is populated by the indexer from on-disk scanning, so it is not directly attacker-controlled — which is why this is low, not high — but the trust boundary is implicit and undocumented at the serve site, and it diverges from the sibling HLS segment endpoint, which strictly validates its path component (`is_valid_segment_name` enforces `seg-NNNN.ts`, `audiobooks.rs:361-367`). Future ingestion paths (uploads, OPDS imports, user-supplied filenames) could feed this column without the serve handler noticing. The same applies to `get_ebook_file`, which resolves `book_file_path` and `tokio::fs::read`s it directly (`server/src/backend/ebooks.rs:67-90`).

Fix: after the join, canonicalize and assert the result still starts with the canonicalized `library_path`, or reject any filename containing a path separator / `..` at the serve site.

---

### Low Priority — Auth gate's /api/ assumption leaves /opds/* open

Both `require_auth` (`server/src/auth/gate.rs:42-48`) and the `origin_check` CSRF middleware (`server/src/auth/csrf.rs:41-65`) early-return for any path that doesn't start with `/api/`. That is correct today because every protected route lives under `/api/`. But F4.2 OPDS specifies a `/opds/*` catalog surface (`docs/roadmap/4-2-opds.md:9,17`) whose feeds are meant to be scoped to the authenticated user, and Phase 4 Kobo/Kindle sync will add its own path namespace.

As written, mounting any `/opds/*` route inherits zero authentication from the existing middleware — it would be wide open unless each OPDS handler re-implements auth or the gate's prefix is widened. OPDS clients also commonly authenticate with HTTP Basic, which the current extractor (cookie + bearer only, `extractor.rs:52-58`) doesn't understand.

This is a forward-looking note, not a present vulnerability, but it is a load-bearing assumption baked into two middlewares that the roadmap is about to violate. Fix when OPDS lands: generalize the gate to cover the new prefixes (or invert it to a deny-by-default allowlist of public paths) and add a Basic-auth strategy in the extractor rather than special-casing it per handler.
