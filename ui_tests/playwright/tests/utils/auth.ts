import { type APIRequestContext, expect } from "@playwright/test";

// When `OMNIBUS_DEV_SEED_USER` is set (local dev via `just dev-up`), the
// seeded user is the first registered account and therefore admin. Playwright
// must use those credentials so `seedLibrary` and other admin-gated calls
// succeed. On CI where no seed user exists, the fallback `playwright` user
// is the first registered (admin) user instead.
function resolveTestCredentials(): { username: string; password: string } {
  const seed = process.env.OMNIBUS_DEV_SEED_USER;
  if (seed) {
    const [username, password] = seed.split(":");
    if (username && password) return { username, password };
  }
  return { username: "playwright", password: "playwright-test-pw-00" };
}

const { username: TEST_USERNAME, password: TEST_PASSWORD } =
  resolveTestCredentials();

export { TEST_PASSWORD, TEST_USERNAME };

/**
 * Ensure the shared test user is logged in on `request`. Registers the user
 * if registration is open; logs in otherwise. Throws if neither works — the
 * usual fix is to wipe `omnibus.db` and retry so registration re-opens.
 */
export async function ensureLoggedIn(
  request: APIRequestContext,
): Promise<void> {
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

/**
 * Mint a bearer session for the shared test user and return the raw token.
 * Used by `globalSetup` to provision a token for the undici-backed `request`
 * fixture — Playwright's `APIRequestContext` does not honor Chromium's
 * `http://localhost` secure-context exception, so the Secure cookie from
 * `ensureLoggedIn` is dropped on every undici-side call. Bearer tokens ride
 * in `Authorization: Bearer` and are not subject to Secure, matching the
 * mobile-client auth path the server already supports in production.
 *
 * Assumes the shared test user already exists (`ensureLoggedIn` runs first
 * in `globalSetup`).
 */
export async function loginBearer(request: APIRequestContext): Promise<string> {
  const resp = await request.post("/api/auth/login", {
    data: {
      username: TEST_USERNAME,
      password: TEST_PASSWORD,
      client_kind: "bearer",
    },
  });
  expect(resp.status(), "bearer login failed").toBe(200);
  const body = (await resp.json()) as { token?: string };
  if (!body.token) {
    throw new Error(
      "bearer login: response missing `token` — server did not honor client_kind",
    );
  }
  return body.token;
}
