## Playwright E2E Test Review

### Overall Technical Score: 62/100

> Coverage 18/25 — strong layout/action coverage across most flows, but the reader's already-shipped P0 progress POST has zero E2E proof and several mutating surfaces (author inline-edit, photo upload UI, detail 404s) lack action or error paths. Speed & parallelism 12/25 — a shared single-DB server with uncapped `fullyParallel` workers, five file-scope serial band-aids, blanket `networkidle` waits, and N full 156 MB re-indexes drag wall-clock and invite contention. Fixture & seed quality 11/20 — read-then-write seeds race-clobber siblings, the settings happy path repoints the live library to `/tmp`, and three hand-synced artifacts drift with no mechanical guard. Selector hygiene 9/15 — pervasive raw `locator()` CSS-class coupling violates the documented hierarchy. Flakiness risk 12/15 — real cross-spec races plus a 5s mutation timeout tighter than the suite's own poll budgets.

---

### High Priority — Every data spec re-indexes the full corpus

Fourteen flow specs call `seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length)` in `beforeAll` (verified: 14 spec files; two further specs — listen, mini-dock — seed the *audiobook* tree via `seedAudiobookLibrary`, so they don't re-index the EPUB corpus this finding is about). Each POSTs the fixtures path to `/api/rpc/settings`, and `rpc_save_settings` unconditionally posts a worker `Scan` task on *every* write — the body comment is explicit that this is deliberate (`frontend/src/rpc.rs:156-168`). The worker does not coalesce or dedup pending tasks: `Worker::post` always allocates a fresh id and `tokio::spawn`s (`db/src/worker/queue.rs:80-155`), and `run()` re-acquires the resource lock and re-runs `reindex()`, which always does a full stat-walk of the tree before the diff skips unchanged-file parses. So N specs pointed at the identical path each enqueue and serially execute a full re-walk of all 49 EPUBs / 156 MB — including the 81 MB Count of Monte Cristo that `seed.ts`'s 45s poll budget (`seed.ts:98-101`) exists to cover. With `fullyParallel:true` and no per-worker DB (`playwright.config.ts:16-18`), wall-clock cost is N serialized scans per shard, and each CI shard re-pays the cold index on its first seed. The ~21 public-domain EPUBs (the bulk of the 156 MB) dominate this cost yet buy little E2E signal: the flows assert against the synthetic `generated/` fixtures; the public-domain set's value is parser robustness, already covered by `db/tests/public_domain_epubs.rs`.

Fix direction: (1) seed once per server in `globalSetup` (it already runs once and writes auth state) so the canonical library indexes a single time and per-spec `beforeAll` degrades to a cheap count assert. (2) Point the default seed at `test_data/epubs/generated` only (`seedLibrary` already takes an explicit path) and move the heavy public-domain set into one dedicated "indexes a real-world library" spec, or leave it to the Rust parser test. (3) Best long-term: a direct-insert seed RPC or a pre-indexed SQLite snapshot loaded at boot so flows never pay EPUB-parse + cover-thumbnail cost. (4) Independently, add pending-scan dedup in the worker (skip if an identical `Scan{path}` is queued/running) — it benefits production too.

---

### High Priority — Settings test repoints library to /tmp

`settings.spec.ts` "saves library paths" (`settings.spec.ts:28-55`) performs a real, un-intercepted `POST /api/rpc/settings` repointing `ebook_library_path` to `/tmp/omnibus-test-ebooks` (an empty, nonexistent dir). Unlike the error-path test (lines 57-88, which `page.route`-fulfills a 500 and never reaches the server), the happy path mutates real server state and per `rpc.rs:159-167` kicks worker `Scan`/`ScanAudiobooks` tasks at those empty paths. `rpc_get_ebooks` scopes its query to whatever `ebook_library_path` is currently configured (`rpc.rs:206-229`), so while this test holds the path at `/tmp`, any concurrently-running landing/search/discovery/browse spec calling `GET /api/rpc/ebooks` reads `/tmp` and gets zero fixture books — the exact condition `fetchBookUuidByTitle` throws on ("no seeded book with title …", `ebooks.ts:27-31`).

`settings.spec.ts` declares no serial mode, has no `beforeAll` seed, and no `afterAll`/`afterEach` restore (verified). Under `fullyParallel` this is a live race and the single most likely source of cross-spec flake; even serially it leaves the DB pointed at `/tmp` until some other spec's `beforeAll` re-seeds — i.e. seeding is doubling as crash recovery. Fix direction: never issue a real library-repointing write from a parallel spec. Best: intercept the happy-path POST with `page.route` the same way the error path does — assert the request payload + "Settings saved." status while fulfilling a canned 200 without touching server state (the test only needs the network contract + status line). Failing that, move settings-mutating tests to a dedicated serial project that restores the canonical library in `afterAll`.

---

### High Priority — Read-then-write seeds race-clobber sibling specs

To avoid wiping each other's data, `seedLibrary` (when no audiobook path is passed) and `seedAudiobookLibrary` both do a check-then-act: `GET /api/rpc/settings`, then `POST` both paths preserving the other half (`seed.ts:43-64`, `71-92`). The mitigating comment ("so parallel specs don't wipe each other's data", `seed.ts:43-44`) only narrows the window; the GET and POST are not atomic. Under `fullyParallel:true` with one shared DB, two `beforeAll`s can interleave: spec A reads (`audiobook=null`), spec B writes `audiobook=fixtures`, spec A then writes `audiobook=null` — silently clobbering B's audiobook seed; the reverse nulls out the ebook path mid-run. This compounds with the settings finding and with mixed ebook+audiobook specs (`book_detail`, `merge`) that flip both paths repeatedly.

Fix direction: eliminate the shared-mutable-settings dependency. Seed both libraries once per shard in `globalSetup` with no per-spec re-pointing, so specs never write settings and there is nothing to race. A spec that genuinely needs a different library shape should run against an isolated server/DB (or a per-worker sqlite file) rather than mutating the shared one.

---

### High Priority — Shared single DB, serial mode as band-aid

`playwright.config.ts` sets `fullyParallel:true` with no `workers` cap and no `webServer` block (lines 11-18, comment 24-32) — every worker drives one externally-managed server against one on-disk DB. All state is global: library path, reading progress, author photos, ignored authors, merge guards. To paper over the resulting bleed the suite has accreted five `test.describe.configure({mode:'serial'})` / `describe.serial` blocks (verified file-scope in `metadata_edit`, `merge`, `author_delete`, `author_photo`, `book_detail`) plus per-test cleanup (`restoreDeletedAuthor`, undo-as-cleanup, `afterEach` photo DELETE). These serial markers wrap whole files, so read-only layout tests in those files are needlessly serialized too, inflating wall-clock time; and the workaround is fragile — any new spec that mutates shared state without opting into serial mode, or any reorder, reintroduces races. `04-playwright` documents none of this.

Fix direction: the durable fix is per-worker isolation — give each worker its own DB+server (a port-walking harness per worker, or a per-worker sqlite file via query param), or seed through a direct-insert RPC keyed per-test instead of re-pointing one global `ebook_library_path`. Once seeding is isolated/idempotent, most serial markers can drop; where true isolation isn't yet feasible, narrow `serial` to a nested `describe` around only the mutating tests so layout/read tests stay parallel. Document the constraint in `04-playwright` either way.

---

### High Priority — Reader flow lacks progress and error coverage

`04-playwright` mandates an action+error test per user action and `expectMutation` around every mutating request. `reader.spec.ts` contains only two layout/navigation tests and zero `expectMutation` / `page.route` calls (`reader.spec.ts:17-55`, grep-confirmed). Yet the reader already POSTs CFI reading progress to `/api/rpc/progress` (`frontend/src/pages/reader/mod.rs:200-212`) and renders a bookmark affordance (`mod.rs:568`, `data-testid="reader-bookmark"`, also asserted at `reader.spec.ts:33`). The listen flow exercises the analogous audio progress POST happy path *and* a forced-500 (`listen.spec.ts:193-271`), reconciling via `/api/rpc/progress/get`; the reader exercises neither. Progress sync is the Phase 2 P0 acceptance criterion (`docs/roadmap/2-1-progress-sync.md`, a unified POST for epub+audio), so EPUB progress persistence — already shipping — has no E2E proof, and a regression where the reader stops POSTing CFI progress would ship green.

Fix direction: add reader specs mirroring `listen.spec` — drive the EPUB progress write through `expectMutation` against `/api/rpc/progress` with the `epub_cfi` payload, add a forced-500 error path asserting the reader stays mounted, and add a bookmark add/remove mutation. At minimum cover the progress POST now, since the endpoint already ships.

---

### High Priority — Tag-cloud tests depend on uncontrolled metadata

The synthetic EPUB generator emits zero `dc:subject` — `buildOpf` (`make_epub.ts`) writes title/language/publisher/date/creators/series but never a subject, and `EpubInput` (`make_epub.ts:22-38`) has no `tags` field. Verified by unzip: `generated/alpha.epub` carries zero `dc:subject`, while `public_domain/{dracula,moby_dick,pride_and_prejudice}.epub` carry several each ("Horror tales", "Gothic fiction", "Whaling -- Fiction", "Love stories", …). Yet `discovery.spec.ts` asserts the tag cloud is populated: "renders the tag cloud layout" checks `.tag-cloud-item` count > 0 (lines 180-182) and "tag cloud items show counts" asserts a numeric count > 0 (lines 188-192). These pass only because the committed public-domain EPUBs incidentally carry subjects nobody on the team controls or pins — swapping/removing a public-domain fixture silently changes the tag surface, and the tests can never assert a specific tag name or count. With ratings/journaling and shared shelves on the roadmap (`docs/roadmap/3-2`, `3-5`) leaning on a real tag/subject taxonomy, this is a coverage hole blocking deterministic assertions.

Fix direction: add a `tags` field to `EpubInput`, emit `dc:subject` in `buildOpf`, give two or three generated fixtures known overlapping subjects (e.g. "Science", "History"), record them in `FIXTURE_BOOKS`, and rewrite the discovery tag tests to assert specific tag names and exact counts. Keep the public-domain set for parser robustness only.

---

### Medium Priority — networkidle gate is slow and poll-flaky

`gotoReady` — the shared nav helper used by essentially every test — always does `page.goto` then `page.waitForLoadState("networkidle")` (`nav.ts:10-13`). `networkidle` (500ms of zero in-flight requests) is explicitly discouraged by Playwright and fights apps that poll. This app polls: `WorkerStatusIndicator` hits `/api/rpc/worker_status` at 1 Hz (`rpc.rs:172-195`, comment "Polled at 1 Hz") and the listen page polls, so on any page mounting a poller `networkidle` may never cleanly settle within budget, and where it does it adds a fixed ~500ms+ tax to all ~122 tests. The `04-playwright` "Waits" rule bans `waitForTimeout` and mandates `expect.poll`/auto-waiting; `networkidle` is the same anti-pattern in a different hat (two specs already reach past it — `browse_indices.spec.ts:142` notes "hydration may lag networkidle" and re-waits).

Fix direction: replace the blanket `networkidle` with a deterministic hydration signal — wait for a known post-hydration landmark element / `data-hydrated` attribute, or `expect(landmarkTestId).toBeVisible()` per page — so each `gotoReady` waits on actual readiness, not network quiescence. Speeds the suite and removes a poll-induced flake source.

---

### Medium Priority — Coverage gaps on shipped mutating surfaces

Several shipped flows have layout/read coverage but no mutation or no error path, contrary to `04-playwright`'s "every mutation asserted" + "error path per action". (1) The inline-edit Authors cell (`landing_inline_edit.spec.ts:121-147`) renders the chip editor and Escapes out but never commits an author change through `expectMutation` — only the title cell save+error is covered, so the author chip-editor save/error path is untested. (2) `author_photo.spec.ts` drives PUT/DELETE photo exclusively through the API `request` fixture (lines 75, 111, 122), never through the browser upload UI that actually exists (the author page has a URL/upload/scan modal, `frontend/src/pages/author.rs:158-160`), and has no forced 4xx/5xx path — so the admin upload UI and its failure rendering are unverified end-to-end. (3) Author and series *detail* pages have no error-path test for a missing/unknown id, unlike `listen.spec.ts:277-284` which covers an unknown audiobook uuid 404.

Fix direction: add an author-cell inline save + forced-500 to `landing_inline_edit`; add a UI-driven photo upload + a forced-error path to `author_photo`; add a 404/unknown-id render assertion for `/authors/:id` and `/series/:id`.

---

### Medium Priority — Raw locator() CSS coupling violates hierarchy

`04-playwright` ranks `locator(...)` last and says class/XPath usage signals the markup needs a role/label/testid. Many specs bind to styling-implementation class names: `nav.breadcrumb` (`browse_indices.spec.ts:25,112`; `discovery.spec.ts:72,150`), `.bd-series-link` (`discovery:124`), `article.series-card` (`discovery:161`), `.tag-cloud-item` / `.tag-cloud-count` (`discovery:180,189`), `.me-chip-item` (`metadata_edit:57`), `div.disc-avatar` / `img.disc-avatar--photo` (`author_photo:61,64,88,92,126,127`), `div.atrium` (`theme_toggle:26,38,59`), `.sort-th[aria-sort='descending']` (`landing:76`). A refactor that renames `.series-card` or `.disc-avatar` breaks these tests with no behavioral change — exactly the coupling the rule forbids. (Data-attribute filters like `button[data-format='epub']`, `button.lib-chip[data-value=…]`, and `#letter-L` are real UI contracts and are defensible.)

Fix direction: add stable `data-testid`s — or a `role=navigation` name on the breadcrumb — to the rsx markup and switch these locators over, per the rule's step 4. Several pages already half-do this, so the inconsistency is the tell.

---

### Medium Priority — Exact full-library counts are brittle

`landing.spec.ts` asserts the EPUB format chip text equals `FIXTURE_BOOKS.length` exactly (line 100, `toContainText`) and the EPUB-filtered row set equals it exactly (line 104, `toHaveCount`). These are the only exact-equality full-size assertions; every other landing assertion uses `toBeGreaterThanOrEqual` (lines 92, 122, 142) precisely because the shared server may carry extra rows (audiobooks ride the same combined `/api/rpc/ebooks` query per `seed.ts:31-35`) or be perturbed by a concurrent merge/settings write. The inconsistency between line 100/104 (exact) and 92/122/142 (gte) in the same file signals the author already knew the shared server isn't safe for exact totals. The exact assertions also silently assume `FIXTURE_BOOKS` stays hand-synced with the on-disk EPUB count (50 entries: 28 generated + ~21 public-domain), so adding/removing a fixture without updating the table hard-fails here rather than at the source.

Fix direction: once seeding is centralized and deterministic (see the shared-reseed finding), exact counts are fine; until then scope the chip assertion to the ebook-only subset or use a stable lower bound, and derive the expected EPUB count from the table rather than the combined total.

---

### Medium Priority — No mechanical guard against fixture drift

Three artifacts must stay hand-synced — `make_epub.ts` `FIXTURES` (28 entries), the committed `generated/*.epub` (28 files), and the `FIXTURE_BOOKS` assertion table (50 entries: 28 generated + 21 public-domain, grep-confirmed `slug:` count = 50). Nothing enforces agreement: the READMEs just instruct humans to update both and regenerate, and the only existing guard is the Rust parser test (`db/tests/public_domain_epubs.rs`), not anything in the Playwright layer. A drift (generator entry edited but `.epub` not regenerated, or table updated without the file) surfaces as an opaque mid-test failure ("row for slug X should be visible", `ebooks.ts:65-67`), not a clear sync error. The audiobook side is looser: `AUDIOBOOK_BOOKS` hard-pins part counts/titles for LibriVox files no generator produces (`audiobooks.ts`), so a rename drifts silently. Regeneration is also ad hoc: two manual `pnpm exec tsx` invocations documented only in READMEs, with no dedicated `justfile` recipe (the package now also exposes `lint`/`typecheck`/`format` scripts and a `just lint-ts`, but nothing that regenerates fixtures).

Fix direction: (1) add a cheap CI/regen check that re-runs the deterministic `make_epub.ts` (it advertises byte-stable output, `make_epub.ts:8`) to a temp dir and asserts byte-identical committed files, plus that every `FIXTURE_BOOKS` generated entry has a matching `FIXTURES` entry and on-disk file. (2) Add `justfile` recipes (e.g. `just fixtures-regen` running both generators + the drift check; `just e2e-seed` wrapping `seed.ts`) so the suite is reproducible from a cold checkout and the "seed once globally" fix has one home.

---

### Medium Priority — Fixture model lacks per-user state

The fixture model is purely library metadata (titles/authors/series/publisher/date/language/cover for EPUBs; title/author/format/parts for audiobooks). It has no notion of per-(user, book) state: no reading progress, ratings, journal entries, shelves, or multi-user data — and the EPUB/MP3 generators structurally cannot express it, since that data lives in the DB (`fixtures/epubs.ts:11-38`, `audiobooks.ts`). The roadmap commits to ratings & journaling (3-2), shared shelves (3-5), stats (3-4), and progress sync (2-1), all per-user. When they land, every E2E will have to create this state inline via RPC per test (slow, contention-prone on the shared server) or the suite needs a new seeding layer. There is also no generated fixture pair the indexer auto-attaches into one multi-format book: `merge.spec.ts:24-41` explicitly notes "No fixture pair shares a normalized (title, author), so the indexer's auto-attach leaves all of them as separate rows", so the dual-format book is constructed by a manual merge and `book_detail`'s "Listen" secondary CTA depends on that merge having run.

Fix direction: design a DB-level seed helper now (a test-only RPC, or a fixtures JSON the server loads) that can stamp ratings/progress/shelf membership for the seeded user, and add at least one generated EPUB+MP3 pair sharing a normalized (title, author) so the indexer auto-attaches a genuine multi-format book without a manual merge.

---

### Low Priority — expectMutation timeout tighter than poll budgets

`api.ts:32-35` hard-codes a 5000ms timeout on `waitForRequest` for every mutating call, with no per-call override and no URL/method in the timeout message. Several mutations fire after debounced/post-hydration effects (merge candidate search is debounced; metadata save fires after a dirty-state settle; the listen progress callback runs after init). On the CI runner that `seed.ts` itself describes as taking 27-36s to index while competing with scan jobs (`seed.ts:98-101`), a starved worker can exceed 5s between click and request, failing in `waitForRequest` with an opaque message rather than at a meaningful assertion. The listen and thumbnails specs already learned the runner is slow and widened their polls to 10s/30s (`listen.spec.ts:39,171`; `thumbnails.spec.ts:75`); `expectMutation` didn't get the same treatment.

Fix direction: make the timeout configurable per call (default ~10-15s to match those polls) and surface the URL/method in the timeout error. Low-to-medium urgency for the current short suite; rises as device-sync/OPDS flows lengthen runs.

---

### Low Priority — Deprecated fetchBookIdByTitle shim still in use

`ebooks.ts:38-43` exports `fetchBookIdByTitle` as an explicitly `@deprecated` alias of `fetchBookUuidByTitle`, but the deprecated name is still imported and called in three live specs — `discovery.spec.ts:3,121`; `landing_inline_edit.spec.ts:4,77`; and `metadata_edit.spec.ts`, which uses it ~13 times. In `metadata_edit` the local is named `id` throughout even though it holds a uuid (e.g. `const id = await fetchBookIdByTitle(...)` then navigates `/books/${id}/edit`). This obscures that the route is uuid-keyed — which matters because `book_url_stability.spec.ts` exists specifically to defend uuid-keying — and the live callers block removal of the shim.

Fix direction: replace the call sites with `fetchBookUuidByTitle` and a `uuid` local, then delete the alias, finishing the migration the deprecation comment announced.

---

### Low Priority — Bearer token and storageState never refresh

`globalSetup` mints one cookie storageState and one bearer token once at suite start and writes them to `.auth/` (`globalSetup.ts:23-36`). storageState reuse is correct and avoids per-test logins. But the bearer token is read once per spec from disk (`fixtures/test.ts:15-34`) with no refresh path and no 401-retry in `auth.ts`; if a run outlives the session/bearer TTL, every request-fixture call across remaining specs 401s with no recovery, surfacing as a confusing cascade of seed failures rather than an auth error. Latent for today's short suite, but the roadmap's device-sync/Kobo/OPDS flows will lengthen runs.

Fix direction: low urgency — document the assumption now; when run length grows, add a re-login-on-401 wrapper around the request fixture (or extend the test user's session TTL) so an expired token self-heals.

---

### Low Priority — Generated EPUBs share one cover and chapter

Every cover-bearing generated EPUB embeds the identical 68-byte 1×1 transparent PNG (`make_epub.ts:384`), and every chapter is the same one-paragraph "Synthetic test content." stub with a single-entry nav doc (`make_epub.ts:450-466`). This is fine for cover-present-vs-absent and metadata assertions, but it caps what the thumbnail pipeline and reader can be tested against: thumbnails from a 1×1 source can't catch aspect-ratio/downscale/WebP regressions, and the reader (`docs/roadmap/2-2`, `2-4b`) renders effectively empty bodies, so pagination, multi-chapter TOC navigation, and text-selection/annotation flows have nothing to exercise — `reader.spec.ts:17-55` consequently asserts only chrome/URL.

Fix direction: add a couple of richer generated fixtures — one EPUB with a real multi-color cover at a typical book aspect ratio (so thumbnail output is meaningfully assertable) and one with several lorem-text chapters and a multi-entry nav doc (so reader pagination/TOC/selection get real content). Keep the minimal ones for the metadata table.
