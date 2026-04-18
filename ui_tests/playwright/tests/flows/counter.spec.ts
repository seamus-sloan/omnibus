import { expect, test } from "../fixtures/test";

test("renders the counter page layout", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator("h1")).toContainText("Counter");
});

test("clicking increment updates the displayed value", async ({ page }) => {
  await page.goto("/");

  const current = page.locator("#current-value");
  const before = Number(await current.innerText());

  await page.locator("#increment-button").click();

  await expect
    .poll(async () => Number(await current.innerText()))
    .toBeGreaterThanOrEqual(before + 1);
});
