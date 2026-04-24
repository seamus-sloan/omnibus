import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { TEST_PASSWORD, TEST_USERNAME } from "../utils/auth";
import { gotoReady } from "../utils/nav";

// These specs test the login flow itself, so they start without the shared
// session cookie that globalSetup wrote.
test.use({ storageState: { cookies: [], origins: [] } });

test("renders the login page layout", async ({ page }) => {
  await gotoReady(page, "/login");

  await expect(page.getByRole("heading", { level: 1, name: "Log in" })).toBeVisible();
  await expect(page.getByLabel("Username")).toBeVisible();
  await expect(page.getByLabel("Password")).toBeVisible();
  await expect(page.getByRole("button", { name: "Log in" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Register" })).toBeVisible();
});

test("renders the register page layout", async ({ page }) => {
  await page.goto("/register");

  await expect(
    page.getByRole("heading", { level: 1, name: "Create an account" }),
  ).toBeVisible();
  await expect(page.getByLabel("Username")).toBeVisible();
  await expect(page.getByLabel("Password")).toBeVisible();
  await expect(page.getByRole("button", { name: "Create account" })).toBeVisible();
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
