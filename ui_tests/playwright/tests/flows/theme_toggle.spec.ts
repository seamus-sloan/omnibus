import { expect, test } from "../fixtures/test";
import { gotoReady } from "../utils/nav";

// F1.6 Atrium theme toggle. The theme controls now live inside the user-menu
// dropdown (Dark / Black / Light / Sepia segmented control). The web build
// persists the choice under `omn.theme` in localStorage and applies it in a
// post-hydration `use_effect` so SSR markup stays deterministic.
//
// Notes for future test authors:
//   - Each Playwright test gets a fresh storage state (only the auth cookie
//     survives — see `auth.setup.ts`). localStorage starts empty per test,
//     so the cold-load assertion below tests the genuine "no persisted
//     value" path.
//   - We assert the `data-theme` attribute on `.atrium` rather than a
//     specific computed style. The attribute is the contract; the variable
//     values are an implementation detail tested at the CSS level.

async function openUserMenu(page: import("@playwright/test").Page) {
  await page.getByTestId("user-menu-trigger").click();
}

test("renders the dark theme on first paint", async ({ page }) => {
  await gotoReady(page, "/");

  const root = page.locator("div.atrium").first();
  await expect(root).toBeVisible();
  await expect(root).toHaveAttribute("data-theme", "dark");

  await openUserMenu(page);
  await expect(page.getByTestId("theme-dark")).toBeVisible();
  await expect(page.getByTestId("theme-light")).toBeVisible();
});

test("toggles dark to light, persists across reload", async ({ page }) => {
  await gotoReady(page, "/");

  const root = page.locator("div.atrium").first();
  await expect(root).toHaveAttribute("data-theme", "dark");

  await openUserMenu(page);
  await page.getByTestId("theme-light").click();
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

test("toggles dark to black, persists across reload", async ({ page }) => {
  await gotoReady(page, "/");

  const root = page.locator("div.atrium").first();
  await expect(root).toHaveAttribute("data-theme", "dark");

  await openUserMenu(page);
  await page.getByTestId("theme-black").click();
  await expect(root).toHaveAttribute("data-theme", "black");

  const stored = await page.evaluate(() => window.localStorage.getItem("omn.theme"));
  expect(stored).toBe("black");

  // Reload: SSR renders `dark`, then the post-hydration effect reads
  // localStorage and flips to `black`. Poll for the follow-up render.
  await page.reload();
  await expect.poll(async () => root.getAttribute("data-theme")).toBe("black");
});

test("clicking light then dark flips back and clears divergence from default", async ({ page }) => {
  await gotoReady(page, "/");

  const root = page.locator("div.atrium").first();

  // Menu stays open between clicks — selecting a theme does not auto-close
  // the panel (the user might want to toggle multiple settings in sequence).
  await openUserMenu(page);
  await page.getByTestId("theme-light").click();
  await expect(root).toHaveAttribute("data-theme", "light");

  await page.getByTestId("theme-dark").click();
  await expect(root).toHaveAttribute("data-theme", "dark");

  const stored = await page.evaluate(() => window.localStorage.getItem("omn.theme"));
  expect(stored).toBe("dark");
});
