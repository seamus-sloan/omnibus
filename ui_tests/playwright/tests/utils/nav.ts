import type { Page } from "@playwright/test";
import { expect } from "../fixtures/test";

// Navigate and wait until the Dioxus WASM client has hydrated. The fullstack
// server SSRs the markup (button + initial value) before the WASM bundle
// finishes loading, so a raw `page.goto` followed by an immediate click fires
// against un-hydrated DOM — the native click succeeds but no rsx onclick
// handler is attached yet, so no API request goes out. `networkidle` blocks
// until the WASM download and the initial server-function fetches settle.
export async function gotoReady(page: Page, path: string): Promise<void> {
  await page.goto(path);
  await page.waitForLoadState("networkidle");
}

// Asserts the shared top-nav is present with the expected links. Used by every
// flow's layout test so we catch nav regressions in one place.
export async function expectNavVisible(page: Page): Promise<void> {
  const nav = page.getByRole("navigation");
  await expect(nav).toBeVisible();
  await expect(nav.getByRole("link", { name: "Home" })).toBeVisible();
  await expect(nav.getByRole("link", { name: "Settings" })).toBeVisible();
}
