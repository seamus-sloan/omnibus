# 04 — Playwright E2E conventions

These rules exist so every flow is tested the same way. Don't diverge without updating this file first.

## Chromium comes from Nix, not npm

The `playwright-driver.browsers` package in [flake.nix](../../flake.nix) provides the browser bundle, and the shellHook exports `PLAYWRIGHT_BROWSERS_PATH` into the Nix store. Do **not** run `npx playwright install` — it would re-download Chromium into `~/Library/Caches/ms-playwright/` and diverge from the flake.

`@playwright/test` is pinned with a tilde range (`~1.58.0`) so npm stays on the same minor as nixpkgs. When bumping the version, update both together since each Playwright minor expects a specific Chromium build number.

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

## Mobile E2E

Not yet implemented. The mobile crate is Dioxus Native (not a WebView), so Playwright cannot reach it. When added, this will be a separate track under `ui_tests/` (likely Appium + WebdriverIO, or Maestro) and will require stable accessibility ids on interactive elements in `mobile/src/`.
