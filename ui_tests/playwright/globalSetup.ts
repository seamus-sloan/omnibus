import { request as apiRequest, type FullConfig } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";

import { ensureLoggedIn } from "./tests/utils/auth";

export const STORAGE_STATE_PATH = resolve(__dirname, ".auth", "storage.json");

export default async function globalSetup(config: FullConfig): Promise<void> {
  const baseURL = config.projects[0]?.use?.baseURL ?? "http://127.0.0.1:3000";
  const ctx = await apiRequest.newContext({ baseURL });
  try {
    await ensureLoggedIn(ctx);
    mkdirSync(dirname(STORAGE_STATE_PATH), { recursive: true });
    await ctx.storageState({ path: STORAGE_STATE_PATH });
  } finally {
    await ctx.dispose();
  }
}
