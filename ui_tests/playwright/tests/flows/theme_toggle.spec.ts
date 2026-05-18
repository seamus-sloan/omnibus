import { expect, test } from "../fixtures/test";
import { gotoReady } from "../utils/nav";

// F1.6 Atrium theme toggle. Lives on the top-nav (web) and flips the
// `data-theme` attribute on the `.atrium` wrapper div emitted by
// `AtriumRoot`. The web build persists the choice under `omn.theme` in
// localStorage and applies it in a post-hydration `use_effect` so SSR
// markup stays deterministic.
//
// Notes for future test authors:
//   - Each Playwright test gets a fresh storage state (only the auth cookie
//     survives — see `auth.setup.ts`). localStorage starts empty per test,
//     so the cold-load assertion below tests the genuine "no persisted
//     value" path.
//   - We assert the `data-theme` attribute on `.atrium` rather than a
//     specific computed style. The attribute is the contract; the variable
//     values are an implementation detail tested at the CSS level.

test("renders the dark theme on first paint", async ({ page }) => {
  await gotoReady(page, "/");

  const root = page.locator("div.atrium").first();
  await expect(root).toBeVisible();
  await expect(root).toHaveAttribute("data-theme", "dark");

  const toggle = page.getByTestId("theme-toggle");
  await expect(toggle).toBeVisible();
  await expect(toggle).toHaveAttribute("aria-label", "Toggle theme");
});

test("toggles dark to light, persists across reload", async ({ page }) => {
  await gotoReady(page, "/");

  const root = page.locator("div.atrium").first();
  await expect(root).toHaveAttribute("data-theme", "dark");

  await page.getByTestId("theme-toggle").click();
  await expect(root).toHaveAttribute("data-theme", "light");

  const stored = await page.evaluate(() => window.localStorage.getItem("omn.theme"));
  expect(stored).toBe("light");

  // Reload: the SSR layer still renders `dark`, then the post-hydration
  // effect reads localStorage and flips to `light`. Poll the attribute so
  // we wait for that single follow-up render instead of asserting on the
  // pre-effect frame.
  await page.reload();
  await expect.poll(async () => root.getAttribute("data-theme")).toBe("light");
});

test("clicking twice flips back to dark and clears divergence from default", async ({ page }) => {
  await gotoReady(page, "/");

  const root = page.locator("div.atrium").first();
  const toggle = page.getByTestId("theme-toggle");

  await toggle.click();
  await expect(root).toHaveAttribute("data-theme", "light");
  await toggle.click();
  await expect(root).toHaveAttribute("data-theme", "dark");

  const stored = await page.evaluate(() => window.localStorage.getItem("omn.theme"));
  expect(stored).toBe("dark");
});
