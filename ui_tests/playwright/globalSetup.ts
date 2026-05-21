import { request as apiRequest, type FullConfig } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

import { ensureLoggedIn, loginBearer } from "./tests/utils/auth";

export const STORAGE_STATE_PATH = resolve(__dirname, ".auth", "storage.json");
export const BEARER_TOKEN_PATH = resolve(__dirname, ".auth", "bearer.txt");

export default async function globalSetup(config: FullConfig): Promise<void> {
  // Pull `baseURL`, `extraHTTPHeaders`, and `storageState` from the merged
  // project `use` so the writer here can't drift from what the rest of the
  // suite reads. `extraHTTPHeaders` matters because `ensureLoggedIn` makes
  // POST requests that would 403 against `origin_check` once those endpoints
  // ever require an Origin header; pulling them from config keeps the rule
  // in one place.
  const projectUse = config.projects[0]?.use ?? {};
  const baseURL = projectUse.baseURL ?? "http://127.0.0.1:3000";
  const extraHTTPHeaders = projectUse.extraHTTPHeaders;
  const storageStatePath =
    typeof projectUse.storageState === "string" ? projectUse.storageState : STORAGE_STATE_PATH;
  const ctx = await apiRequest.newContext({ baseURL, extraHTTPHeaders });
  try {
    await ensureLoggedIn(ctx);
    mkdirSync(dirname(storageStatePath), { recursive: true });
    await ctx.storageState({ path: storageStatePath });

    // Mint a bearer session for the same user. The browser fixture (`page`)
    // rides the cookie persisted to storageState above; the undici-backed
    // `request` fixture rides the bearer below (see fixtures/test.ts) so
    // Secure-by-default cookies don't drop session state on non-browser HTTP
    // calls — Playwright's undici client doesn't honor Chromium's
    // localhost-secure-context exception.
    const token = await loginBearer(ctx);
    mkdirSync(dirname(BEARER_TOKEN_PATH), { recursive: true });
    writeFileSync(BEARER_TOKEN_PATH, token, { mode: 0o600 });
  } finally {
    await ctx.dispose();
  }
}
