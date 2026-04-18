import { expect, test } from "@playwright/test";

test("settings page renders at /settings", async ({ page }) => {
  await page.goto("/settings");
  await expect(page.locator("h1")).toHaveText("Settings");
});
