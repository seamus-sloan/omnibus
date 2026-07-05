import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { expectNavVisible, gotoReady } from "../utils/nav";

// F4.3 — the per-user `/account` page hosts the Send-to-Kindle destination
// address. The seeded session (global setup) is an authed admin, which is a
// superset of the plain-user surface this page needs.

const emailInput = (page: Page) => page.getByTestId("kindle-email-input");

test("renders the account page layout", async ({ page }) => {
  await page.goto("/account");

  await expect(page.getByRole("heading", { level: 1, name: "Account" })).toBeVisible();
  await expect(page.getByTestId("account-kindle-card")).toBeVisible();
  await expect(emailInput(page)).toBeVisible();
  await expect(page.getByTestId("kindle-email-save")).toBeVisible();
  await expect(page.getByTestId("kindle-email-connected")).toBeAttached();
  await expectNavVisible(page);
});

test("saves the Kindle email and shows a success status", async ({ page }) => {
  await gotoReady(page, "/account");

  const address = "reader@kindle.com";
  await emailInput(page).fill(address);

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/account/kindle-email",
      expectedBody: { email: address },
      expectedStatus: 200,
    },
    async () => page.getByTestId("kindle-email-save").click(),
  );

  await expect(page.getByTestId("kindle-email-status")).toHaveText("Kindle email saved.");
  await expect(page.getByTestId("kindle-email-status")).toHaveClass(/success/);
});

test("shows an error status when saving the Kindle email fails", async ({ page }) => {
  await gotoReady(page, "/account");

  await emailInput(page).fill("reader@kindle.com");

  await page.route("**/api/rpc/account/kindle-email", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({ status: 500, contentType: "text/plain", body: "forced failure" });
    }
    return route.continue();
  });

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/account/kindle-email",
      expectedStatus: 500,
    },
    async () => page.getByTestId("kindle-email-save").click(),
  );

  await expect(page.getByTestId("kindle-email-status")).toHaveClass(/error/);
});
