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
// Per-run unique suffix so a run that dies before its cleanup delete doesn't
// 409 the next run's create against the shared dev server.
const TEMP_USER = `e2e_usermgmt_${Date.now()}`;
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

test("a backdrop click during an in-flight delete does not dismiss the modal or lose the result", async ({
  page,
}) => {
  // #1484 — DeleteUserModal used to sit on a hand-rolled `ModalShell` whose
  // backdrop had no busy gate at all, so a stray click while the delete was
  // in flight closed the modal before the admin saw the outcome. It now
  // shares `ConfirmModal`, which refuses to dismiss while `busy`.
  await gotoReady(page, USERS);

  const tempName = `e2e_backdrop_${Date.now()}`;
  await page.getByTestId("users-new").click();
  await page.getByTestId("new-user-username").fill(tempName);
  await page.getByTestId("new-user-password").fill(TEMP_PASSWORD);
  await expectMutation(
    page,
    { method: "POST", url: "/api/users", expectedStatus: 201 },
    async () => page.getByTestId("new-user-submit").click(),
  );
  const row = table(page).locator("tr", { hasText: tempName });
  await expect(row).toBeVisible();

  await row.getByRole("button", { name: "Delete" }).click();
  const modal = page.getByTestId("users-delete-modal");
  await expect(modal).toBeVisible();

  // Delay the DELETE response so the modal is provably still busy when the
  // backdrop click lands mid-request.
  await page.route("**/api/users/*", async (route) => {
    if (route.request().method() !== "DELETE") return route.continue();
    await new Promise((resolve) => setTimeout(resolve, 400));
    return route.continue();
  });

  try {
    await expectMutation(
      page,
      { method: "DELETE", url: "/api/users/", expectedStatus: 204 },
      async () => {
        await page.getByTestId("delete-user-confirm").click();
        // Click the backdrop well away from the centered panel while the
        // delete is still in flight (the route above delays the response) —
        // the modal must stay open and the request must still complete.
        await modal.click({ position: { x: 5, y: 5 } });
        await expect(modal).toBeVisible();
      },
    );
  } finally {
    await page.unroute("**/api/users/*");
  }

  await expect(modal).toHaveCount(0);
  await expect(row).toHaveCount(0);
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

// ── Self-registration switch ──────────────────────────────────────────
//
// `registration_enabled` is a single global settings row that the whole
// server reads, and `auth.spec.ts` asserts the register page and the login
// page's Register link — both of which key off it. Flipping it for real would
// be visible to those specs the instant it landed (the suite is
// `fullyParallel` against one shared server), and a spec-local restore
// wouldn't help: readers don't take the writers' lock. So these specs stub
// both the read and the write, asserting the network contract and the UI's
// reaction without ever touching shared state. The real persistence path is
// covered by the server integration tests
// (`api_post_registration_*` in `server/src/backend/settings/tests.rs`).

/** Pin the registration-status read so the switch starts from a known value. */
async function stubRegistrationStatus(page: Page, enabled: boolean) {
  await page.route("**/api/auth/registration", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ enabled }),
    });
  });
}

test("reflects the current self-registration setting", async ({ page }) => {
  await stubRegistrationStatus(page, false);
  await gotoReady(page, USERS);

  const toggle = page.getByTestId("registration-toggle");
  await expect(toggle).not.toBeChecked();
  await expect(page.getByTestId("registration-card")).toContainText(
    "Only an administrator can create accounts.",
  );
  await expect(toggle).toBeEnabled();
});

test("turning self-registration on posts the new value and updates the card", async ({
  page,
}) => {
  await stubRegistrationStatus(page, false);
  await page.route("**/api/settings/registration", async (route) => {
    await route.fulfill({ status: 204, body: "" });
  });
  await gotoReady(page, USERS);

  const toggle = page.getByTestId("registration-toggle");
  await expect(toggle).not.toBeChecked();

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/settings/registration",
      expectedBody: { enabled: true },
      expectedStatus: 204,
    },
    async () => toggle.check(),
  );

  await expect(toggle).toBeChecked();
  await expect(page.getByTestId("registration-card")).toContainText(
    "Anyone who can reach this server can create an account.",
  );
  await expect(page.getByTestId("registration-error")).toHaveCount(0);
});

test("a failed save surfaces the error and leaves the switch as it was", async ({
  page,
}) => {
  await stubRegistrationStatus(page, true);
  await page.route("**/api/settings/registration", async (route) => {
    await route.fulfill({
      status: 500,
      contentType: "text/plain",
      body: "set registration enabled failed",
    });
  });
  await gotoReady(page, USERS);

  const toggle = page.getByTestId("registration-toggle");
  await expect(toggle).toBeChecked();

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/settings/registration",
      expectedBody: { enabled: false },
      expectedStatus: 500,
    },
    async () => toggle.uncheck(),
  );

  await expect(page.getByTestId("registration-error")).toContainText(
    "set registration enabled failed",
  );
  // The switch must not claim a state the server rejected.
  await expect(toggle).toBeChecked();
  await expect(page.getByTestId("registration-card")).toContainText(
    "Anyone who can reach this server can create an account.",
  );
});
