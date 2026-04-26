import { request as apiRequest, type FullConfig } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";

import { ensureLoggedIn } from "./tests/utils/auth";

export const STORAGE_STATE_PATH = resolve(__dirname, ".auth", "storage.json");

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
  } finally {
    await ctx.dispose();
  }
}
