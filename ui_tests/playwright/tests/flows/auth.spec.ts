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
  // `exact: true` scopes to the input — the StrengthMeter carries
  // `aria-label="Password strength"` which would otherwise also match.
  await expect(page.getByLabel("Password", { exact: true })).toBeVisible();
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

test("tab from Username focuses Password (skipping the Forgot? link)", async ({ page }) => {
  // The Forgot? link sits visually beside the Password label. It must
  // not intercept the tab path between Username and Password — otherwise
  // a user typing "name <Tab> password <Enter>" lands on the link with
  // their typed password going nowhere, and Enter activates the link
  // (re-navigates /login) instead of submitting.
  await gotoReady(page, "/login");

  await page.getByLabel("Username").focus();
  await page.keyboard.press("Tab");
  await expect(page.getByLabel("Password")).toBeFocused();
});

test("Enter from Password submits the login form", async ({ page }) => {
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
    async () => page.getByLabel("Password").press("Enter"),
  );

  await expect(page).toHaveURL(/\/$/);
});

test("register routes a password error to the password Field", async ({ page }) => {
  await gotoReady(page, "/register");

  await page.route("**/api/auth/register", async (route) => {
    await route.fulfill({
      status: 400,
      contentType: "text/plain",
      body: "password is too short",
    });
  });

  // `exact: true` scopes the password label to the input; the
  // StrengthMeter's `aria-label="Password strength"` would otherwise
  // make this a strict-mode violation.
  await page.getByLabel("Username").fill("new-user");
  await page.getByLabel("Password", { exact: true }).fill("short");

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

  // Prove the routing: the password input flips to aria-invalid, and
  // the error message renders inside the password Field (not the
  // top-level Banner, which would also carry role=alert).
  const passwordInput = page.getByLabel("Password", { exact: true });
  await expect(passwordInput).toHaveAttribute("aria-invalid", "true");
  await expect(page.getByLabel("Username")).toHaveAttribute("aria-invalid", "false");
  // `<Field>` exposes `data-testid="<input_id>-field"` on its wrapper —
  // scope the alert to that field rather than walking the DOM with
  // XPath (per `.claude/rules/04-playwright.md`).
  const passwordField = page.getByTestId("register-password-field");
  await expect(passwordField.getByRole("alert")).toContainText("password is too short");
  await expect(
    page.getByRole("button", { name: /fix to continue/i }),
  ).toBeDisabled();
});
