import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { expectNavVisible, gotoReady } from "../utils/nav";

// F4.3 — the Send-to-Kindle destination address lives in the Account section
// of the unified Settings sidebar (#1324, #1345); the legacy standalone
// `/account` page now redirects there (mirroring `/logs`) so there is a
// single, consistently-chromed Account surface regardless of entry point.
// The seeded session (global setup) is an authed admin, which is a superset
// of the plain-user surface this page needs.
const ACCOUNT = "/settings?section=account";

const emailInput = (page: Page) => page.getByTestId("kindle-email-input");

test("renders the account section layout", async ({ page }) => {
  await gotoReady(page, ACCOUNT);

  await expect(
    page.getByRole("heading", { level: 2, name: "Account" }),
  ).toBeVisible();
  await expect(page.getByTestId("account-kindle-card")).toBeVisible();
  await expect(emailInput(page)).toBeVisible();
  await expect(page.getByTestId("kindle-email-save")).toBeVisible();
  await expect(page.getByTestId("kindle-email-connected")).toBeAttached();
  await expectNavVisible(page);
});

test("the legacy /account route redirects into the Account section", async ({
  page,
}) => {
  await gotoReady(page, "/account");

  await expect(page).toHaveURL(/\/settings\??$/);
  await expect(
    page.getByRole("heading", { level: 2, name: "Account" }),
  ).toBeVisible();
  await expect(page.getByTestId("account-kindle-card")).toBeVisible();
});

test("saves the Kindle email and shows a success status", async ({ page }) => {
  await gotoReady(page, ACCOUNT);

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

  await expect(page.getByTestId("kindle-email-status")).toHaveText(
    "Kindle email saved.",
  );
  await expect(page.getByTestId("kindle-email-status")).toHaveClass(/success/);
});

test("shows an error status when saving the Kindle email fails", async ({
  page,
}) => {
  await gotoReady(page, ACCOUNT);

  await emailInput(page).fill("reader@kindle.com");

  await page.route("**/api/rpc/account/kindle-email", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 500,
        contentType: "text/plain",
        body: "forced failure",
      });
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
