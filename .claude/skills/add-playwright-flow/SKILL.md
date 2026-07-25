---
name: add-playwright-flow
description: Recipe for adding a new Playwright E2E spec under ui_tests/playwright/tests/flows/. Triggers when the user asks to add an E2E test, add a Playwright spec, or cover a new UI flow end-to-end.
---

# Add a Playwright flow

Full conventions live in [04-playwright.md](../../rules/04-playwright.md). This is the shortened recipe.

## 1. Create the spec file

One file per user flow: `ui_tests/playwright/tests/flows/<flow-name>.spec.ts`.

Import `test` and `expect` from the fixtures file, never from `@playwright/test` directly:

```ts
import { test, expect } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { expectNavVisible } from "../utils/nav";
```

## 2. Write a layout test

One per spec, no user actions:

```ts
test("renders the <page> layout", async ({ page }) => {
  await page.goto("/<path>");
  await expectNavVisible(page);
  await expect(page.getByRole("heading", { name: "<page title>" })).toBeVisible();
});
```

## 3. Write one action test per user action

Happy path and error path each get their own test.

**Happy path:**

```ts
test("<verb> <object> and <expected outcome>", async ({ page }) => {
  await page.goto("/<path>");
  await page.getByLabel("<input label>").fill("<value>");

  await expectMutation(
    page,
    { method: "POST", url: "/api/<path>", expectedBody: { ... }, expectedStatus: 200 },
    async () => page.getByRole("button", { name: "<button>" }).click(),
  );

  await expect(page.getByRole("status")).toHaveText("<success text>");
});
```

**Error path:** intercept the mutating route with `page.route` + `route.fulfill({ status: 500, ... })` before triggering the action. Still wrap in `expectMutation` to verify the request fired. Assert the UI surfaces the error.

## 4. Selector preference order

1. `getByRole` — buttons, headings, links, form landmarks, live regions.
2. `getByText` — visible text not tied to a role.
3. `getByLabel` — form inputs. Add the label in SSR markup if missing.
4. `getByTestId` — only when nothing else fits. Add `data-testid` alongside `id` in the Dioxus rsx.
5. `locator` — last resort. Never XPath.

## 5. Flow-specific helpers

Helpers used only by this spec (e.g. `fillSettingsForm`) live inside the spec file. Only cross-flow helpers go under `utils/`.

## 6. Run

```bash
just dev-up                            # idempotent, identity-checked, fast on reuse
source .claude/runtime/env.sh          # picks up PLAYWRIGHT_BASE_URL for the chosen port
cd ui_tests/playwright
pnpm exec playwright test              # pnpm, not npm/npx
```

Before pushing, lint + typecheck the TS: `just lint-ts` (Biome check + `tsc --noEmit`), or `pnpm run lint` / `pnpm run typecheck` from `ui_tests/playwright`.

If you need to drive the same page manually (for snapshots / debugging while iterating on the spec), use the [`ui-validate`](../ui-validate/SKILL.md) skill — same server, same login, Claude Preview (`mcp__Claude_Preview__preview_*`) by default.

## 7. End-of-session

Run [99-end-of-session.md](../../rules/99-end-of-session.md).
