import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { gotoReady } from "../utils/nav";

// The user menu replaces the old top-bar cluster (ThemeToggle, Settings
// link, Log out button) with a single avatar trigger that opens a dropdown.
// Most rows in the dropdown are forward-looking stubs (disabled <a>); real
// wiring covers Settings, Sign out, and the Dark/Light theme buttons.

test("renders the user menu trigger after login", async ({ page }) => {
  await gotoReady(page, "/");

  const trigger = page.getByTestId("user-menu-trigger");
  await expect(trigger).toBeVisible();
  await expect(trigger).toHaveAttribute("aria-expanded", "false");

  // The old top-bar Log out button is gone — it now lives inside the menu
  // and should not be reachable while the menu is closed.
  await expect(page.getByTestId("logout-button")).toHaveCount(0);
});

test("opens and closes the user menu", async ({ page }) => {
  await gotoReady(page, "/");

  const trigger = page.getByTestId("user-menu-trigger");
  await trigger.click();
  await expect(trigger).toHaveAttribute("aria-expanded", "true");

  // Real actions are present.
  await expect(page.getByRole("link", { name: "Settings" })).toBeVisible();
  await expect(page.getByTestId("logout-button")).toBeVisible();
  await expect(page.getByTestId("theme-dark")).toBeVisible();
  await expect(page.getByTestId("theme-light")).toBeVisible();

  // Close via the transparent scrim (click outside the panel).
  await page.getByTestId("user-menu-scrim").click();
  await expect(page.getByTestId("logout-button")).toHaveCount(0);

  // Re-open and close via ESC. Dispatch the keypress directly to the
  // panel locator so the test doesn't race the onmounted focus call.
  await trigger.click();
  const panel = page.getByTestId("user-menu-panel");
  await expect(panel).toBeVisible();
  await panel.press("Escape");
  await expect(page.getByTestId("logout-button")).toHaveCount(0);
});

test("Settings link routes to the settings page", async ({ page }) => {
  await gotoReady(page, "/");
  await page.getByTestId("user-menu-trigger").click();
  await page.getByRole("link", { name: "Settings" }).click();
  await expect(page).toHaveURL(/\/settings$/);
});

test("Sign out clears the session and routes to /login", async ({ page }) => {
  // Mock /api/auth/logout instead of letting it hit the real endpoint:
  //   1. A real logout would invalidate the globalSetup-minted session
  //      that every parallel test relies on — they'd all redirect to
  //      /login mid-run with a clobbered cookie.
  //   2. The realistic alternative (fresh login per test) burns the
  //      /api/auth/login per-IP rate-limit budget that auth.spec.ts
  //      already saturates in CI.
  // The backend logout path is covered by `server::auth` integration
  // tests; this spec is the UI contract — request fires, UI navigates.
  await page.route("**/api/auth/logout", async (route) => {
    await route.fulfill({ status: 200, contentType: "text/plain", body: "" });
  });

  await gotoReady(page, "/");
  await page.getByTestId("user-menu-trigger").click();

  await expectMutation(
    page,
    { method: "POST", url: "/api/auth/logout", expectedStatus: 200 },
    async () => page.getByTestId("logout-button").click(),
  );

  await expect(page).toHaveURL(/\/login$/);
});
