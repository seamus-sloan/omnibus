# 04 — Playwright E2E conventions

These rules exist so every flow is tested the same way. Don't diverge without updating this file first.

For live preview validation during development (not E2E specs), see the [ui-validate](../skills/ui-validate/SKILL.md) skill — it covers port-walking server bring-up, login state, and rebuild detection.

## Toolchain — pnpm, TypeScript 7, Biome

The Playwright project (`ui_tests/playwright`) uses **pnpm** as its package manager (never `npm`), **TypeScript 7** (the native `tsc`), and **Biome** as its linter + formatter. pnpm is Nix-provided (`.#web`/`.#e2e` shells) and its version is pinned via the `packageManager` field; `.npmrc` disables pnpm's version self-management and `pnpm-workspace.yaml` allowlists esbuild's build script. Commit `pnpm-lock.yaml`, `pnpm-workspace.yaml`, `.npmrc`, and `biome.json`.

- Install / run: `pnpm install`, `pnpm exec playwright test`, `pnpm exec tsx tools/…`.
- Lint + typecheck: `pnpm run lint` (Biome), `pnpm run typecheck` (`tsc --noEmit`), or `just lint-ts` for both. Enforced in CI by `.github/workflows/ts-lint.yml` (the `🔒 TS Lint Required` gate).
- Biome config is [biome.json](../../ui_tests/playwright/biome.json): double quotes, 2-space indent, 80-col, recommended lint rules + organize-imports. `noNonNullAssertion` is **off** — the specs use `!` on indexed access (`FIXTURE_BOOKS[0]!`) because `tsconfig.json` sets `noUncheckedIndexedAccess`; the two would otherwise fight. When Biome offers a `!`→`?.` "fix", reject it: it changes types and breaks the typecheck.

## Chromium comes from Nix, not npm

The `playwright-driver.browsers` package in [flake.nix](../../flake.nix) provides the browser bundle, and the shellHook exports `PLAYWRIGHT_BROWSERS_PATH` into the Nix store. Do **not** run `pnpm exec playwright install` — it would re-download Chromium into `~/Library/Caches/ms-playwright/` and diverge from the flake.

`@playwright/test` is pinned with a tilde range (`~1.59.0`) so pnpm stays on the same minor as nixpkgs. When bumping the version, update both together since each Playwright minor expects a specific Chromium build number.

## Reporters — `list` + `junit`

[`playwright.config.ts`](../../ui_tests/playwright/playwright.config.ts) configures both a `list` reporter (console) and a `junit` reporter (`results.xml`). The JUnit output feeds Codecov Test Analytics — don't drop it. CI ([`e2e.yml`](../../.github/workflows/e2e.yml)) overrides the reporter on the CLI (`--reporter=list,blob,junit`), adds `blob` for the merged HTML report, and sets `PLAYWRIGHT_JUNIT_OUTPUT_NAME` per shard before uploading via `codecov/codecov-action@v5` (`report_type: test_results`).

## Style — functional helpers + fixtures

Never page-object classes. Import `test` and `expect` from `tests/fixtures/test.ts` (not directly from `@playwright/test`) so shared fixtures apply uniformly. Factor reusable selectors and actions into plain functions.

## Selectors — semantic first, `locator()` last, never XPath

Preference order:

1. `page.getByRole(...)` — buttons, headings, links, form landmarks, live regions (`status`, `alert`). Also form buttons: `getByRole("button", { name: "Save" })`.
2. `page.getByText(...)` — visible text not tied to a role.
3. `page.getByLabel(...)` — form inputs with a `<label for=...>`. Add a proper label in SSR markup rather than reaching for a test id.
4. `page.getByTestId(...)` — only when no role/text/label fits. Add `"data-testid": "..."` (alongside the existing `id`) to the Dioxus rsx markup. Keep names stable and meaningful — they're part of the UI contract.
5. `page.locator(...)` — last resort.

Never use XPath. If you want XPath, the SSR markup probably needs a role, label, or testid added instead.

## Structure — one file per flow

Under `tests/flows/`, each `*.spec.ts` contains:

1. **One layout test** (`renders the <page> layout`) asserting the destination page's structure: key elements visible, shared nav present (via `expectNavVisible` from `utils/nav.ts`). No user actions.
2. **One or more action tests**, one per user action, covering happy path and error path. Action tests drive the UI, assert network contracts, then assert UI state.

Flow-specific helpers (e.g. `fillSettingsForm`) live inside the flow's spec file. Only cross-flow helpers go to `utils/`.

## Waits — `expect.poll` and auto-waiting only

No `waitForTimeout`. If the DOM is going to change, poll for it. If a request must complete before asserting, `await` the response via `expectMutation` from `utils/api.ts`.

## Network — every mutation must be asserted

Wrap every mutating request (POST/PUT/PATCH/DELETE) in `expectMutation`:

```ts
await expectMutation(
  page,
  { method: "POST", url: "/api/settings", expectedBody: {...}, expectedStatus: 200 },
  async () => page.getByRole("button", { name: "Save" }).click(),
);
```

It arms `waitForRequest`/`waitForResponse`, runs the action, checks payload and status, and guarantees the test waited for the response before any subsequent UI assertion. Reads (GET) are not asserted unless the assertion depends on their data.

## Error paths — force failures with `page.route`

Intercept the mutating route and `route.fulfill({ status: 500, ... })` before triggering the action, then still use `expectMutation` to verify the request fired with the expected payload and observed the forced status. Assert the UI surfaces the error (status text, error class, unchanged state).

## Example

```ts
test("saves library paths and shows a success status", async ({ page }) => {
  await page.goto("/settings");
  await page.getByLabel("Ebook Library Path").fill(path);

  await expectMutation(
    page,
    { method: "POST", url: "/api/settings", expectedBody: { ... }, expectedStatus: 200 },
    async () => page.getByRole("button", { name: "Save" }).click(),
  );

  await expect(page.getByRole("status")).toHaveText("Settings saved.");
});
```

See `ui_tests/playwright/tests/flows/settings.spec.ts` for the full version.

## Seeding — point the server at fixtures, then poll

Specs that need real DB content (e.g. landing) call `seedLibrary(request, fixturesDir(), expectedCount)` from `tests/utils/seed.ts` in a `test.beforeAll`. It POSTs the absolute fixtures path to `/api/rpc/settings`, then polls `GET /api/rpc/ebooks` until the indexer has surfaced the expected number of books — `rpc_save_settings` kicks the reindex off in a `tokio::spawn`, so the POST response alone isn't enough.

The committed fixtures live under `test_data/epubs/generated/` and are produced by `ui_tests/playwright/tools/make_epub.ts` (run via `pnpm exec tsx`); CBZ comic fixtures come from the sibling `tools/make_cbz.ts`. The single source of truth for expected per-row metadata is `tests/fixtures/epubs.ts` (`FIXTURE_BOOKS`); audiobooks have the parallel `tests/fixtures/audiobooks.ts` (`AUDIOBOOK_BOOKS`, generated by `tools/make_audiobook.ts`). When you add a fixture, regenerate the files and update the matching table — plus the Rust mirror in `db/tests/fixture_epubs.rs` — in the same change.

**Never mutate a fixture another spec reads.** The suite is `fullyParallel` against one shared server, so a merge, delete, or reindex is globally visible the moment it lands. A cross-worker lock (`withLock` in `utils/lock.ts`) only serializes the writers — readers don't take it, so serializing writers alone does *not* protect readers. Give the mutating spec its own fixture instead: `MERGE_PRIMARY` / `MERGE_SECONDARY` in `tests/fixtures/audiobooks.ts` are reserved for the `/api/rpc/merge-books` specs and read by nothing else. Likewise `frankenstein` / `great-gatsby` in `tests/fixtures/epubs.ts` are reserved for the reader progress-restore specs — saved position is per-(user, book) server state, so any other spec opening them in the reader would race those assertions. The two CBZ fixtures (`aurora-station-01` / `aurora-station-02`) are reserved for `comic_reader.spec.ts` on the same grounds: `-01` receives that spec's progress writes and `-02` must stay progress-free (its layout test asserts a page-1 open). `standalone-ocean` is reserved for `landing_inline_edit.spec.ts`'s Tags-cell tests, which hold a `subjects` override on it mid-test (its fixture entry leaves `tags` unasserted for the same reason). Pick an author that appears in no other fixture too — `shelves.spec.ts` asserts exact author-scoped match counts.

**Dual-format fixtures.** One book ("Immersive Voyage") exists as both an EPUB and an audiobook that share a normalized (title, author), so the indexer auto-attaches them into a single dual-format book — the precondition for the Immersive Read CTA (`immersive.spec.ts`). It's the *only* intentional (title, author) collision across the two libraries; keep every other pair distinct. A spec that seeds **both** libraries and asserts an exact combined total must subtract `AUTO_ATTACHED_PAIRS` (`tests/fixtures/dual_format.ts`) from `FIXTURE_BOOKS.length + AUDIOBOOK_BOOK_COUNT` — each attached pair is two files but one book.

## Mobile E2E

The iOS surface is the native SwiftUI app (`omnibus-ios/`); its UI coverage is the `omnibusUITests` XCUITest suite (`just ios-test-ui`, CI's `ios-tests.yml`) — not Playwright. The `mobile/` crate is the Android **hybrid app** — a thin native shell (`mobile/src/main.rs`) hosting the system WebView rendering the shared `omnibus-frontend` markup with `features = ["mobile"]` — and currently has no E2E lane (Playwright can reach the Android WebView over CDP, but none is wired up).
