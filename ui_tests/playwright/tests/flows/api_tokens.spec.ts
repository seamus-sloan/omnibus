import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { expectNavVisible, gotoReady } from "../utils/nav";

// Settings → API Tokens (#2313): per-user long-lived `omni_…` bearers with a
// create-once secret, a name/created/last-used listing, and per-row revoke.
// Tokens are per-user state on the shared seeded admin, so every test names
// its tokens uniquely and revokes what it created — counts are never
// asserted, only presence/absence of this test's own rows.
const SETTINGS_API_TOKENS = "/settings?section=api-tokens";

const nameInput = (page: Page) => page.getByTestId("api-token-name-input");
const createButton = (page: Page) => page.getByTestId("api-token-create");
const secretField = (page: Page) => page.getByTestId("api-token-secret");
const status = (page: Page) => page.getByTestId("api-tokens-status");

const rowFor = (page: Page, name: string) =>
  page.getByTestId("api-token-row").filter({ hasText: name });

async function createToken(page: Page, name: string): Promise<void> {
  await nameInput(page).fill(name);
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/api-tokens/create",
      expectedBody: { name },
      expectedStatus: 200,
    },
    async () => createButton(page).click(),
  );
}

async function revokeToken(page: Page, name: string): Promise<void> {
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/api-tokens/revoke",
      expectedStatus: 200,
    },
    async () =>
      rowFor(page, name).getByRole("button", { name: "Revoke" }).click(),
  );
  await expect(rowFor(page, name)).toHaveCount(0);
}

test("renders the API Tokens section layout", async ({ page }) => {
  await gotoReady(page, SETTINGS_API_TOKENS);

  await expectNavVisible(page);
  await expect(page.getByTestId("api-tokens-card")).toBeVisible();
  await expect(page.getByRole("heading", { name: "API Tokens" })).toBeVisible();
  await expect(nameInput(page)).toBeVisible();
  await expect(createButton(page)).toBeVisible();
  // No secret surface exists before a create — the secret only ever renders
  // once, immediately after minting.
  await expect(secretField(page)).toHaveCount(0);
});

test("creates a token, shows the secret exactly once, and revokes it", async ({
  page,
}) => {
  const name = `e2e once ${Date.now()}`;
  await gotoReady(page, SETTINGS_API_TOKENS);

  await createToken(page, name);

  // The secret is displayed now — an omni_-prefixed bearer — alongside the
  // new row in the listing.
  await expect(secretField(page)).toBeVisible();
  const secret = await secretField(page).inputValue();
  expect(secret).toMatch(/^omni_/);
  await expect(rowFor(page, name)).toBeVisible();

  // After a reload the secret is unrecoverable: the row persists, the
  // secret surface does not.
  await gotoReady(page, SETTINGS_API_TOKENS);
  await expect(rowFor(page, name)).toBeVisible();
  await expect(secretField(page)).toHaveCount(0);

  await revokeToken(page, name);
});

test("shows an error and keeps the list unchanged when creation fails", async ({
  page,
}) => {
  const name = `e2e fail ${Date.now()}`;
  await gotoReady(page, SETTINGS_API_TOKENS);

  await page.route("**/api/rpc/api-tokens/create", (route) =>
    route.fulfill({ status: 500, body: "internal server error" }),
  );
  await nameInput(page).fill(name);
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/api-tokens/create",
      expectedBody: { name },
      expectedStatus: 500,
    },
    async () => createButton(page).click(),
  );

  await expect(status(page)).toBeVisible();
  await expect(status(page)).toHaveClass(/error/);
  await expect(secretField(page)).toHaveCount(0);
  await expect(rowFor(page, name)).toHaveCount(0);
});

// ── Hosted MCP endpoint toggle (#2314) ──────────────────────────────
// Renders in the same section, admin-only (the seeded user is an admin).
// The toggle is instance-wide state: the action test flips it on and
// restores it off (the shipped default) before finishing, and nothing else
// in the suite reads it — no other spec touches /mcp.

const mcpToggle = (page: Page) => page.getByTestId("mcp-toggle");
const mcpState = (page: Page) => page.getByTestId("mcp-toggle-state");

test("renders the MCP endpoint toggle card for an admin", async ({ page }) => {
  await gotoReady(page, SETTINGS_API_TOKENS);

  await expect(page.getByTestId("mcp-toggle-card")).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Hosted MCP endpoint" }),
  ).toBeVisible();
  await expect(mcpState(page)).toBeVisible();
  // The card waits for the status read before offering the switch.
  await expect(mcpToggle(page)).toBeEnabled();
});

test("enables and disables the MCP endpoint", async ({ page }) => {
  await gotoReady(page, SETTINGS_API_TOKENS);
  await expect(mcpToggle(page)).toBeEnabled();

  try {
    // The dev instance ships with the toggle off; flip on, then restore.
    await expectMutation(
      page,
      {
        method: "POST",
        url: "/api/settings/mcp",
        expectedBody: { enabled: true },
        expectedStatus: 204,
      },
      async () => mcpToggle(page).click(),
    );
    await expect(page.getByTestId("mcp-toggle-status")).toHaveClass(/success/);
    await expect(mcpState(page)).toContainText("Enabled");

    await expectMutation(
      page,
      {
        method: "POST",
        url: "/api/settings/mcp",
        expectedBody: { enabled: false },
        expectedStatus: 204,
      },
      async () => mcpToggle(page).click(),
    );
    await expect(mcpState(page)).toContainText("Disabled");
  } finally {
    // Failure-safe restore of the instance-wide default: a mid-test failure
    // must not leave /mcp enabled for the rest of the suite. Direct API
    // write (idempotent) — the UI's state is unknowable after a failure.
    await page.request.post("/api/settings/mcp", {
      data: { enabled: false },
    });
  }
});
