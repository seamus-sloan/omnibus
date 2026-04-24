import { type APIRequestContext } from "@playwright/test";
import { expect } from "../fixtures/test";

// A single shared test user. The server gates all `/api/*` routes behind
// authentication and disables registration after the first user is created,
// so every spec shares one seeded user. The globalSetup registers it (or logs
// in if registration is closed) and writes the session cookie to
// `.auth/storage.json`, which Playwright then loads for every test context.
export const TEST_USERNAME = "playwright";
export const TEST_PASSWORD = "playwright-test-pw-00";

/**
 * Ensure the shared test user is logged in on `request`. Registers the user
 * if registration is open; logs in otherwise. Throws if neither works — the
 * usual fix is to wipe `omnibus.db` and retry so registration re-opens.
 */
export async function ensureLoggedIn(request: APIRequestContext): Promise<void> {
  const registerResp = await request.post("/api/auth/register", {
    data: {
      username: TEST_USERNAME,
      password: TEST_PASSWORD,
    },
  });
  if (registerResp.status() === 200) return;
  // 409 (username taken) or 403 (registration disabled) → fall back to login.
  if (registerResp.status() !== 409 && registerResp.status() !== 403) {
    throw new Error(
      `auth setup: /api/auth/register returned ${registerResp.status()} ${await registerResp.text()}`,
    );
  }
  const loginResp = await request.post("/api/auth/login", {
    data: {
      username: TEST_USERNAME,
      password: TEST_PASSWORD,
    },
  });
  expect(
    loginResp.status(),
    `auth setup: /api/auth/login returned ${loginResp.status()} — wipe omnibus.db and rerun if the existing ${TEST_USERNAME} row has a different password`,
  ).toBe(200);
}
