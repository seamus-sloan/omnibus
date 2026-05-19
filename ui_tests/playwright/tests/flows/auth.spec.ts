import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { TEST_PASSWORD, TEST_USERNAME } from "../utils/auth";
import { gotoReady } from "../utils/nav";

// These specs test the login flow itself, so they start without the shared
// session cookie that globalSetup wrote.
test.use({ storageState: { cookies: [], origins: [] } });

test("renders the login page layout", async ({ page }) => {
  await gotoReady(page, "/login");

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
  await expect(page.getByRole("meter")).toBeVisible();
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

  await expect(page.getByRole("alert")).toContainText("invalid credentials");
  await expect(page).toHaveURL(/\/login$/);
});

test("shows an error when submitting an empty form", async ({ page }) => {
  await gotoReady(page, "/login");

  // No mutation expected — the client rejects before sending.
  await page.getByRole("button", { name: "Log in" }).click();

  await expect(page.getByRole("alert")).toContainText(/username and password/i);
});

test("register routes a password error to the password Field", async ({ page }) => {
  await page.goto("/register");

  await page.route("**/api/auth/register", async (route) => {
    await route.fulfill({
      status: 400,
      contentType: "text/plain",
      body: "password is too short",
    });
  });

  await page.getByLabel("Username").fill("new-user");
  await page.getByLabel("Password").fill("short");

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/auth/register",
      expectedStatus: 400,
    },
    async () =>
      page.getByRole("button", { name: "Create account" }).click(),
  );

  await expect(page.getByRole("alert")).toContainText("password is too short");
  await expect(
    page.getByRole("button", { name: /fix to continue/i }),
  ).toBeDisabled();
});
