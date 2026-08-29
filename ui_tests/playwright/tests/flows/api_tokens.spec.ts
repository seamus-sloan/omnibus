import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { expectNavVisible, gotoReady } from "../utils/nav";

// Settings → API Tokens (#2313, redesigned in #2330): per-user long-lived
// `omni_…` bearers with a create-once secret hand-off (copy affordances +
// explicit dismiss), a token table with status pills and `omni_…xxxx`
// identifiers, in-row rename, and in-row revoke confirmation. Tokens are
// per-user state on the shared seeded admin, so every test names its tokens
// uniquely and revokes what it created — counts are never asserted, only
// presence/absence of this test's own rows.
const SETTINGS_API_TOKENS = "/settings?section=api-tokens";

const nameInput = (page: Page) => page.getByTestId("api-token-name-input");
const createButton = (page: Page) => page.getByTestId("api-token-create");
const secretPanel = (page: Page) => page.getByTestId("api-token-secret-field");
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

// Revoke is two-step: the row's Revoke swaps in a confirmation row, and only
// its "Revoke token" fires the mutation.
async function revokeToken(page: Page, name: string): Promise<void> {
  await rowFor(page, name).getByRole("button", { name: "Revoke" }).click();
  const confirm = page.getByTestId("api-token-revoke-confirm");
  await expect(confirm).toBeVisible();
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/api-tokens/revoke",
      expectedStatus: 200,
    },
    async () => confirm.getByRole("button", { name: "Revoke token" }).click(),
  );
  await expect(rowFor(page, name)).toHaveCount(0);
}

test("renders the API Tokens section layout", async ({ page }) => {
  await gotoReady(page, SETTINGS_API_TOKENS);

  await expectNavVisible(page);
  await expect(page.getByTestId("api-tokens-card")).toBeVisible();
  await expect(page.getByRole("heading", { name: "API Tokens" })).toBeVisible();
  // The head states the live-token count and the account's blast radius —
  // the seeded admin always carries the Admin chip.
  await expect(page.getByTestId("api-tokens-count")).toContainText("live");
  await expect(page.getByTestId("api-tokens-scope")).toContainText("Admin");
  await expect(nameInput(page)).toBeVisible();
  await expect(createButton(page)).toBeVisible();
  // No secret surface exists before a create — the secret only ever renders
  // once, immediately after minting.
  await expect(secretPanel(page)).toHaveCount(0);
});

test("creates a token, hands off the secret exactly once, and revokes it", async ({
  page,
}) => {
  const name = `e2e once ${Date.now()}`;
  await gotoReady(page, SETTINGS_API_TOKENS);

  await createToken(page, name);

  // The hand-off panel is displayed now: the omni_-prefixed secret, its copy
  // button, and the pre-filled `claude mcp add` command carrying the same
  // secret and this instance's origin.
  await expect(secretPanel(page)).toBeVisible();
  const secret = (await secretField(page).textContent()) ?? "";
  expect(secret).toMatch(/^omni_/);
  await expect(page.getByTestId("api-token-secret-copy")).toBeVisible();
  const command = page.getByTestId("api-token-mcp-command");
  await expect(command).toContainText(secret);
  await expect(command).toContainText(`${new URL(page.url()).origin}/mcp`);
  await expect(page.getByTestId("api-token-mcp-command-copy")).toBeVisible();

  // The new row lists the token with its omni_…xxxx identifier and the
  // never-used pill.
  const row = rowFor(page, name);
  await expect(row).toBeVisible();
  await expect(row.getByText(`omni_…${secret.slice(-4)}`)).toBeVisible();
  await expect(row.getByTestId("api-token-pill")).toHaveText("Never used");

  // Dismissing hides the secret for good.
  await page.getByTestId("api-token-secret-dismiss").click();
  await expect(secretPanel(page)).toHaveCount(0);

  // After a reload the secret is unrecoverable: the row persists, the
  // secret surface does not.
  await gotoReady(page, SETTINGS_API_TOKENS);
  await expect(rowFor(page, name)).toBeVisible();
  await expect(secretPanel(page)).toHaveCount(0);

  await revokeToken(page, name);
});

test("cancels an in-row revoke confirmation without revoking", async ({
  page,
}) => {
  const name = `e2e cancel ${Date.now()}`;
  await gotoReady(page, SETTINGS_API_TOKENS);
  await createToken(page, name);
  await page.getByTestId("api-token-secret-dismiss").click();

  await rowFor(page, name).getByRole("button", { name: "Revoke" }).click();
  const confirm = page.getByTestId("api-token-revoke-confirm");
  await expect(confirm).toBeVisible();
  await confirm.getByRole("button", { name: "Cancel" }).click();
  await expect(confirm).toHaveCount(0);
  await expect(rowFor(page, name)).toBeVisible();

  await revokeToken(page, name);
});

test("renames a token in place", async ({ page }) => {
  const name = `e2e rename ${Date.now()}`;
  const renamed = `${name} v2`;
  await gotoReady(page, SETTINGS_API_TOKENS);
  await createToken(page, name);
  await page.getByTestId("api-token-secret-dismiss").click();

  await rowFor(page, name).getByRole("button", { name: "Rename" }).click();
  const input = page.getByTestId("api-token-rename-input");
  await expect(input).toHaveValue(name);
  await input.fill(renamed);
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/api-tokens/rename",
      expectedStatus: 200,
    },
    async () => page.getByTestId("api-token-rename-save").click(),
  );
  await expect(rowFor(page, renamed)).toBeVisible();

  // The rename survives a reload — it persisted, not just re-rendered.
  await gotoReady(page, SETTINGS_API_TOKENS);
  await expect(rowFor(page, renamed)).toBeVisible();

  await revokeToken(page, renamed);
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
  await expect(secretPanel(page)).toHaveCount(0);
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
  // The copyable endpoint URL carries this instance's origin.
  await expect(page.getByTestId("mcp-endpoint-url")).toHaveText(
    `${new URL(page.url()).origin}/mcp`,
  );
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
    await expect(mcpState(page)).toHaveText("enabled");
    await expect(mcpToggle(page)).toHaveAttribute("aria-checked", "true");

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
    await expect(mcpState(page)).toHaveText("disabled");
    await expect(mcpToggle(page)).toHaveAttribute("aria-checked", "false");
  } finally {
    // Failure-safe restore of the instance-wide default: a mid-test failure
    // must not leave /mcp enabled for the rest of the suite. Direct API
    // write (idempotent) — the UI's state is unknowable after a failure.
    await page.request.post("/api/settings/mcp", {
      data: { enabled: false },
    });
  }
});
