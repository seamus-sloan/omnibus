import { expect, test } from "@playwright/test";

test("landing page renders with counter heading", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator("h1")).toContainText("Counter");
});
