import type { Page } from "@playwright/test";
import { expect } from "../fixtures/test";

// Asserts the shared top-nav is present with the expected links. Used by every
// flow's layout test so we catch nav regressions in one place.
export async function expectNavVisible(page: Page): Promise<void> {
  const nav = page.locator("nav.top-nav");
  await expect(nav).toBeVisible();
  await expect(nav.getByRole("link", { name: "Home" })).toBeVisible();
  await expect(nav.getByRole("link", { name: "Settings" })).toBeVisible();
}
