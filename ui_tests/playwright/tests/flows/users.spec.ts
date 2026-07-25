import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { TEST_USERNAME } from "../utils/auth";
import { expectNavVisible, gotoReady } from "../utils/nav";

// Admin user management (#1326). The seeded session is an admin, so it sees the
// Users section. The create/delete round-trip uses a username no other spec or
// fixture touches and removes it at the end, so the shared server is left as it
// was found. The last-admin / non-admin guards are covered by the server
// integration tests (they'd mutate or depend on the shared admin here).
const USERS = "/settings?section=users";
const TEMP_USER = "e2e_usermgmt_temp";
const TEMP_PASSWORD = "Temp-E2E-Pass-9x";

const table = (page: Page) => page.getByTestId("users-table");
const tempRow = (page: Page) =>
  table(page).locator("tr", { hasText: TEMP_USER });

test("renders the users section layout", async ({ page }) => {
  await gotoReady(page, USERS);

  await expect(
    page.getByRole("heading", { level: 2, name: "Users" }),
  ).toBeVisible();
  await expect(page.getByTestId("users-table")).toBeVisible();
  await expect(page.getByTestId("users-new")).toBeVisible();
  // The seeded admin is always present.
  await expect(
    table(page).locator("tr", { hasText: TEST_USERNAME }),
  ).toBeVisible();
  await expectNavVisible(page);
});

test("administrator permission implies and locks the others", async ({
  page,
}) => {
  await gotoReady(page, USERS);
  await page.getByTestId("users-new").click();

  const upload = page.getByTestId("perm-can_upload");
  const edit = page.getByTestId("perm-can_edit");
  await expect(upload).not.toBeChecked();

  await page.getByTestId("perm-is_admin").check();
  // Administrator forces the rest on and disables them.
  await expect(upload).toBeChecked();
  await expect(edit).toBeChecked();
  await expect(upload).toBeDisabled();
  await expect(edit).toBeDisabled();
});

test("creates a user then deletes it", async ({ page }) => {
  await gotoReady(page, USERS);

  // ── Create ──────────────────────────────────────────────
  await page.getByTestId("users-new").click();
  await page.getByTestId("new-user-username").fill(TEMP_USER);
  await page.getByTestId("new-user-password").fill(TEMP_PASSWORD);
  await page.getByTestId("perm-can_upload").check();

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/users",
      expectedBody: {
        username: TEMP_USER,
        password: TEMP_PASSWORD,
        permissions: {
          is_admin: false,
          can_upload: true,
          can_edit: false,
          can_download: true,
        },
      },
      expectedStatus: 201,
    },
    async () => page.getByTestId("new-user-submit").click(),
  );

  await expect(tempRow(page)).toBeVisible();
  await expect(tempRow(page).getByText("Upload")).toBeVisible();

  // ── Delete (cleanup) ────────────────────────────────────
  await tempRow(page).getByRole("button", { name: "Delete" }).click();
  await expect(page.getByTestId("users-delete-modal")).toBeVisible();

  await expectMutation(
    page,
    { method: "DELETE", url: "/api/users/", expectedStatus: 204 },
    async () => page.getByTestId("delete-user-confirm").click(),
  );

  await expect(tempRow(page)).toHaveCount(0);
});

test("surfaces a 409 when creating a duplicate username", async ({ page }) => {
  await gotoReady(page, USERS);
  await page.getByTestId("users-new").click();

  // Collide with the seeded admin — the server rejects before any write.
  await page.getByTestId("new-user-username").fill(TEST_USERNAME);
  await page.getByTestId("new-user-password").fill(TEMP_PASSWORD);

  await expectMutation(
    page,
    { method: "POST", url: "/api/users", expectedStatus: 409 },
    async () => page.getByTestId("new-user-submit").click(),
  );

  await expect(page.getByTestId("users-modal-error")).toContainText(
    "username is already taken",
  );
});
