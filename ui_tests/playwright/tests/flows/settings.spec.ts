import { expect, test } from "../fixtures/test";

test("renders the settings page layout", async ({ page }) => {
  await page.goto("/settings");
  await expect(page.locator("h1")).toHaveText("Settings");
});
