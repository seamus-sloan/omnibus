import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { TEST_PASSWORD } from "../utils/auth";
import { expectNavVisible, gotoReady } from "../utils/nav";

// F4.3 — the Send-to-Kindle destination address lives in the Account section
// of the unified Settings sidebar (#1324, #1345); the legacy standalone
// `/account` page now redirects there (mirroring `/logs`) so there is a
// single, consistently-chromed Account surface regardless of entry point.
// The seeded session (global setup) is an authed admin, which is a superset
// of the plain-user surface this page needs.
const ACCOUNT = "/settings?section=account";

const emailInput = (page: Page) => page.getByTestId("kindle-email-input");
const currentPassword = (page: Page) =>
  page.getByTestId("current-password-input");
const newPassword = (page: Page) => page.getByTestId("new-password-input");
const changeStatus = (page: Page) => page.getByTestId("change-password-status");

test("renders the account section layout", async ({ page }) => {
  await gotoReady(page, ACCOUNT);

  await expect(
    page.getByRole("heading", { level: 2, name: "Account" }),
  ).toBeVisible();
  await expect(page.getByTestId("account-kindle-card")).toBeVisible();
  await expect(emailInput(page)).toBeVisible();
  await expect(page.getByTestId("kindle-email-save")).toBeVisible();
  await expect(page.getByTestId("kindle-email-connected")).toBeAttached();

  // The change-password card sits below the Kindle card in the same section.
  await expect(
    page.getByRole("heading", { level: 2, name: "Password" }),
  ).toBeVisible();
  await expect(page.getByTestId("account-password-card")).toBeVisible();
  await expect(currentPassword(page)).toBeVisible();
  await expect(newPassword(page)).toBeVisible();
  await expect(page.getByTestId("change-password-submit")).toBeVisible();

  // Kobo wireless sync card (#927/#1439).
  await expect(page.getByTestId("kobo-devices-card")).toBeVisible();

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

// Change-password (#1325). The success test mocks the mutation: the suite
// reuses this admin's credentials across every spec, so an actual password
// change would strand later `ensureLoggedIn` calls. The real DB behavior
// (verify current, validate new, stamp `password_changed_at`) is covered by
// the `omnibus-db` unit tests; the rejection tests below hit the live server
// and are safe because both reject *before* the write.

test("changes the password and clears the form on success", async ({
  page,
}) => {
  await gotoReady(page, ACCOUNT);

  await page.route("**/api/rpc/account/change-password", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: "null",
      });
    }
    return route.continue();
  });

  await currentPassword(page).fill(TEST_PASSWORD);
  await newPassword(page).fill("Brand-New-Valid-9x");

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/account/change-password",
      expectedBody: {
        current_password: TEST_PASSWORD,
        new_password: "Brand-New-Valid-9x",
      },
      expectedStatus: 200,
    },
    async () => page.getByTestId("change-password-submit").click(),
  );

  await expect(changeStatus(page)).toHaveText("Password changed.");
  await expect(changeStatus(page)).toHaveClass(/success/);
  await expect(currentPassword(page)).toHaveValue("");
  await expect(newPassword(page)).toHaveValue("");
});

test("rejects a wrong current password and leaves the form untouched", async ({
  page,
}) => {
  await gotoReady(page, ACCOUNT);

  await currentPassword(page).fill("definitely-not-the-password");
  await newPassword(page).fill("Brand-New-Valid-9x");

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/account/change-password",
      expectedBody: {
        current_password: "definitely-not-the-password",
        new_password: "Brand-New-Valid-9x",
      },
      expectedStatus: 500,
    },
    async () => page.getByTestId("change-password-submit").click(),
  );

  await expect(changeStatus(page)).toHaveClass(/error/);
  await expect(changeStatus(page)).toContainText(
    "current password is incorrect",
  );
  // Inputs keep their values so the user can correct just the current field.
  await expect(currentPassword(page)).toHaveValue(
    "definitely-not-the-password",
  );
});

test("rejects an invalid new password", async ({ page }) => {
  await gotoReady(page, ACCOUNT);

  // Correct current password authorizes; the too-short new password fails
  // policy on the server before any write happens.
  await currentPassword(page).fill(TEST_PASSWORD);
  await newPassword(page).fill("short");

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/account/change-password",
      expectedBody: { current_password: TEST_PASSWORD, new_password: "short" },
      expectedStatus: 500,
    },
    async () => page.getByTestId("change-password-submit").click(),
  );

  await expect(changeStatus(page)).toHaveClass(/error/);
});
