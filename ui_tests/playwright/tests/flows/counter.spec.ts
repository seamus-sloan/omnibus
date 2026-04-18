import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { expectNavVisible } from "../utils/nav";

const incrementButton = (page: Page) =>
  page.getByRole("button", { name: "Increment value" });

const currentValue = (page: Page) => page.getByTestId("current-value");

test("renders the counter page layout", async ({ page }) => {
  await page.goto("/");

  await expect(page.getByRole("heading", { level: 1 })).toContainText("Counter");
  await expect(currentValue(page)).toBeVisible();
  await expect(incrementButton(page)).toBeVisible();
  await expectNavVisible(page);
});

test("clicking increment posts to the API and updates the displayed value", async ({ page }) => {
  await page.goto("/");

  const current = currentValue(page);
  await expect(current).not.toHaveText("");
  const before = Number(await current.innerText());

  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/value/increment", expectedStatus: 200 },
    async () => incrementButton(page).click(),
  );

  await expect
    .poll(async () => Number(await current.innerText()))
    .toBeGreaterThanOrEqual(before + 1);
});

test("leaves the displayed value unchanged when the increment API fails", async ({ page }) => {
  await page.goto("/");

  const current = currentValue(page);
  await expect(current).not.toHaveText("");
  const before = await current.innerText();

  await page.route("**/api/rpc/value/increment", (route) =>
    route.fulfill({ status: 500, contentType: "text/plain", body: "forced failure" }),
  );

  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/value/increment", expectedStatus: 500 },
    async () => incrementButton(page).click(),
  );

  await expect(current).toHaveText(before);
});
