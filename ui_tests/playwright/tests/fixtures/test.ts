// Extended `test` / `expect` for the omnibus Playwright suite.
//
// Overrides the default `request` fixture so the undici-backed
// `APIRequestContext` rides bearer auth on protected `/api/*` routes.
// Browsers (`page` fixture) keep cookie auth via storageState. This split
// matches production: web uses Secure cookies, mobile/CLI uses bearer —
// Playwright's undici client is in the mobile/CLI bucket since it doesn't
// honor Chromium's `http://localhost` secure-context exception for Secure
// cookies. The bearer is provisioned by globalSetup.
import { test as base, expect } from "@playwright/test";
import { existsSync, readFileSync } from "node:fs";

import { BEARER_TOKEN_PATH } from "../../globalSetup";

function loadBearerToken(): string | null {
  if (!existsSync(BEARER_TOKEN_PATH)) return null;
  const token = readFileSync(BEARER_TOKEN_PATH, "utf8").trim();
  return token.length > 0 ? token : null;
}

export const test = base.extend({
  request: async ({ playwright, baseURL, extraHTTPHeaders }, use) => {
    const token = loadBearerToken();
    const ctx = await playwright.request.newContext({
      baseURL,
      extraHTTPHeaders: {
        ...extraHTTPHeaders,
        ...(token ? { Authorization: `Bearer ${token}` } : {}),
      },
    });
    await use(ctx);
    await ctx.dispose();
  },
});
export { expect };
