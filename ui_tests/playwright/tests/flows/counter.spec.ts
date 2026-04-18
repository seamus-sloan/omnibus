import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { expectNavVisible } from "../utils/nav";

test("renders the counter page layout", async ({ page }) => {
  await page.goto("/");

  await expect(page.locator("h1")).toContainText("Counter");
  await expect(page.locator("#current-value")).toBeVisible();
  await expect(page.locator("#increment-button")).toBeVisible();
  await expectNavVisible(page);
});

test("clicking increment posts to the API and updates the displayed value", async ({ page }) => {
  await page.goto("/");

  const current = page.locator("#current-value");
  await expect(current).not.toHaveText("");
  const before = Number(await current.innerText());

  await expectMutation(
    page,
    { method: "POST", url: "/api/value/increment", expectedStatus: 200 },
    async () => page.locator("#increment-button").click(),
  );

  await expect
    .poll(async () => Number(await current.innerText()))
    .toBeGreaterThanOrEqual(before + 1);
});

test("leaves the displayed value unchanged when the increment API fails", async ({ page }) => {
  await page.goto("/");

  const current = page.locator("#current-value");
  await expect(current).not.toHaveText("");
  const before = await current.innerText();

  await page.route("**/api/value/increment", (route) =>
    route.fulfill({ status: 500, contentType: "text/plain", body: "forced failure" }),
  );

  await expectMutation(
    page,
    { method: "POST", url: "/api/value/increment", expectedStatus: 500 },
    async () => page.locator("#increment-button").click(),
  );

  await expect(current).toHaveText(before);
});
