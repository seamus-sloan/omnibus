import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { TEST_PASSWORD, TEST_USERNAME } from "../utils/auth";
import { gotoReady } from "../utils/nav";

// These specs test the login flow itself, so they start without the shared
// session cookie that globalSetup wrote.
test.use({ storageState: { cookies: [], origins: [] } });

test("renders the login page layout", async ({ page }) => {
  await gotoReady(page, "/login");

  // F1.6 wraps the form in AuthShell — the form title is now an h2.
  await expect(
    page.getByRole("heading", { level: 2, name: /Welcome back/i }),
  ).toBeVisible();
  await expect(page.getByLabel("Username")).toBeVisible();
  await expect(page.getByLabel("Password")).toBeVisible();
  await expect(
    page.getByLabel("Keep me signed in for 30 days"),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Log in" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Register" })).toBeVisible();
});

test("renders the register page layout", async ({ page }) => {
  await page.goto("/register");

  await expect(
    page.getByRole("heading", { level: 2, name: /Make yourself at home/i }),
  ).toBeVisible();
  await expect(page.getByLabel("Username")).toBeVisible();
  await expect(page.getByLabel("Password")).toBeVisible();
  // StrengthMeter renders role=meter.
  await expect(page.getByRole("meter")).toBeVisible();
  // One checklist row is enough — full coverage is overkill.
  await expect(page.getByText("At least 10 characters")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Create account" }),
  ).toBeVisible();
  await expect(page.getByRole("link", { name: "Log in" })).toBeVisible();
});

test("logs in with correct credentials and redirects to landing", async ({ page }) => {
  await gotoReady(page, "/login");

  await page.getByLabel("Username").fill(TEST_USERNAME);
  await page.getByLabel("Password").fill(TEST_PASSWORD);

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/auth/login",
      expectedStatus: 200,
    },
    async () => page.getByRole("button", { name: "Log in" }).click(),
  );

  await expect(page).toHaveURL(/\/$/);
});

test("shows an error when login credentials are wrong", async ({ page }) => {
  await gotoReady(page, "/login");

  await page.getByLabel("Username").fill(TEST_USERNAME);
  await page.getByLabel("Password").fill("definitely-not-the-password");

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/auth/login",
      expectedStatus: 401,
    },
    async () => page.getByRole("button", { name: "Log in" }).click(),
  );

  // F1.6 routes errors through the Banner primitive; BannerKind::Err
  // renders role=alert and inherits the server's flat error string.
  await expect(page.getByRole("alert")).toContainText("invalid credentials");
  await expect(page).toHaveURL(/\/login$/);
});

test("shows an error when submitting an empty form", async ({ page }) => {
  await gotoReady(page, "/login");

  // No mutation expected — the client rejects before sending.
  await page.getByRole("button", { name: "Log in" }).click();

  await expect(page.getByRole("alert")).toContainText(/username and password/i);
});
