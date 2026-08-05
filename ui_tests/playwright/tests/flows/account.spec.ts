import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { TEST_PASSWORD } from "../utils/auth";
import { expectNavVisible, gotoReady } from "../utils/nav";

// The unified Settings sidebar (#1324, #1345) splits the per-user surfaces
// into their own sections: Account holds the password card, Kindle the
// Send-to-Kindle destination address (F4.3), Kobo the wireless-sync devices
// (#927/#1439). The legacy standalone `/account` page redirects into Account
// (mirroring `/logs`) so there is one consistently-chromed entry point.
// The seeded session (global setup) is an authed admin, which is a superset
// of the plain-user surface these sections need.
const ACCOUNT = "/settings?section=account";
const KINDLE = "/settings?section=kindle";
const KOBO = "/settings?section=kobo";

const emailInput = (page: Page) => page.getByTestId("kindle-email-input");
const currentPassword = (page: Page) =>
  page.getByTestId("current-password-input");
const newPassword = (page: Page) => page.getByTestId("new-password-input");
const changeStatus = (page: Page) => page.getByTestId("change-password-status");
const displayNameInput = (page: Page) => page.getByTestId("display-name-input");
const profileStatus = (page: Page) => page.getByTestId("profile-status");

test("renders the account section layout", async ({ page }) => {
  await gotoReady(page, ACCOUNT);

  await expect(page.getByTestId("settings-nav-account")).toHaveAttribute(
    "aria-current",
    "page",
  );
  // Profile card sits at the top of the section.
  await expect(page.getByTestId("account-profile-card")).toBeVisible();
  await expect(displayNameInput(page)).toBeVisible();
  await expect(page.getByTestId("display-name-save")).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 2, name: "Password" }),
  ).toBeVisible();
  await expect(page.getByTestId("account-password-card")).toBeVisible();
  await expect(currentPassword(page)).toBeVisible();
  await expect(newPassword(page)).toBeVisible();
  await expect(page.getByTestId("change-password-submit")).toBeVisible();

  // Kindle and Kobo have their own sections now — not this one.
  await expect(page.getByTestId("account-kindle-card")).toHaveCount(0);
  await expect(page.getByTestId("kobo-devices-card")).toHaveCount(0);

  await expectNavVisible(page);
});

test("renders the Kindle section layout", async ({ page }) => {
  await gotoReady(page, KINDLE);

  await expect(page.getByTestId("settings-nav-kindle")).toHaveAttribute(
    "aria-current",
    "page",
  );
  await expect(
    page.getByRole("heading", { level: 2, name: "Kindle" }),
  ).toBeVisible();
  await expect(page.getByTestId("account-kindle-card")).toBeVisible();
  await expect(emailInput(page)).toBeVisible();
  await expect(page.getByTestId("kindle-email-save")).toBeVisible();
  await expect(page.getByTestId("kindle-email-connected")).toBeAttached();

  await expectNavVisible(page);
});

test("renders the Kobo section layout", async ({ page }) => {
  await gotoReady(page, KOBO);

  await expect(page.getByTestId("settings-nav-kobo")).toHaveAttribute(
    "aria-current",
    "page",
  );
  await expect(page.getByTestId("kobo-devices-card")).toBeVisible();

  await expectNavVisible(page);
});

test("the legacy /account route redirects into the Account section", async ({
  page,
}) => {
  await gotoReady(page, "/account");

  await expect(page).toHaveURL(/\/settings\??$/);
  await expect(page.getByTestId("account-password-card")).toBeVisible();
});

test("saves the Kindle email and shows a success status", async ({ page }) => {
  await gotoReady(page, KINDLE);

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
  await gotoReady(page, KINDLE);

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

// Display name (profile card). The seeded admin is shared across the parallel
// suite and the display name is globally visible — it relabels the wishlist
// shelf and every journal/rating byline — so the success test resets it in a
// `finally`, and the failure test mocks the response rather than writing.

test("saves the display name and shows a success status", async ({ page }) => {
  await gotoReady(page, ACCOUNT);

  try {
    await displayNameInput(page).fill("Seamus");

    await expectMutation(
      page,
      {
        method: "POST",
        url: "/api/rpc/account/profile",
        expectedBody: { display_name: "Seamus" },
        expectedStatus: 200,
      },
      async () => page.getByTestId("display-name-save").click(),
    );

    await expect(profileStatus(page)).toHaveText("Profile saved.");
    await expect(profileStatus(page)).toHaveClass(/success/);

    // The card re-reads `/api/auth/me` after the save, so the user menu shows
    // the new name without a reload.
    await page.getByTestId("user-menu-trigger").click();
    await expect(page.getByTestId("user-menu-panel")).toContainText("Seamus");
  } finally {
    await page.keyboard.press("Escape");
    await displayNameInput(page).fill("");
    await expectMutation(
      page,
      {
        method: "POST",
        url: "/api/rpc/account/profile",
        expectedBody: { display_name: null },
        expectedStatus: 200,
      },
      async () => page.getByTestId("display-name-save").click(),
    );
  }
});

test("shows an error status when saving the display name fails", async ({
  page,
}) => {
  await gotoReady(page, ACCOUNT);

  await displayNameInput(page).fill("Seamus");

  await page.route("**/api/rpc/account/profile", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 500,
        contentType: "application/json",
        body: JSON.stringify({ message: "forced failure" }),
      });
    }
    return route.continue();
  });

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/account/profile",
      expectedStatus: 500,
    },
    async () => page.getByTestId("display-name-save").click(),
  );

  await expect(profileStatus(page)).toHaveClass(/error/);
});

test("shows an error when the avatar upload fails", async ({ page }) => {
  await gotoReady(page, ACCOUNT);

  await page.route("**/api/account/avatar", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 400,
        contentType: "text/plain",
        body: "unsupported image type",
      });
    }
    return route.continue();
  });

  await page.getByTestId("avatar-file-input").setInputFiles({
    name: "avatar.png",
    mimeType: "image/png",
    buffer: Buffer.from(
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
      "base64",
    ),
  });

  await expect(page.getByTestId("avatar-status")).toBeVisible();
  await expect(page.getByTestId("avatar-status")).toHaveClass(/error/);
});
