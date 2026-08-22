// Book-detail hero chip editors: the admin-only "+ genres" / "+ tags" pills
// that swap the hero's chip lists for the shared ChipEditor and save a
// genres/subjects override on every add.
//
// Both editors write `metadata_overrides` on the target — globally visible
// server state that flips `has_override` — which is why this file owns
// `standalone-glacier` outright (see the fixture comment in
// `tests/fixtures/epubs.ts`).

import type { APIRequestContext } from "@playwright/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "standalone-glacier")!;

// Every test writes/clears overrides on the SAME reserved book, so the
// suite-wide `fullyParallel` would let one test's `clearOverrides` wipe
// another's just-saved chip mid-assertion. Run this file sequentially in
// one worker instead.
test.describe.configure({ mode: "default" });

/** The save endpoint every chip write goes through. */
const OVERRIDES_RPC = /\/api\/rpc\/ebook\/overrides$/;

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

/** Drop every override on the target so each test starts chip-free. */
async function clearOverrides(request: APIRequestContext) {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  const resp = await request.post("/api/rpc/ebook/overrides/delete", {
    data: { uuid },
  });
  expect(resp.status(), "cleanup revert must succeed").toBe(200);
}

test.beforeEach(async ({ request }) => {
  await clearOverrides(request);
});

test.afterAll(async ({ request }) => {
  await clearOverrides(request);
});

test("renders the book detail chip-editor layout", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);
  await expectNavVisible(page);

  // Both pills render even though the book carries no genres or tags —
  // they are the entry point, not a decoration on existing chips.
  await expect(page.getByTestId("bd-add-genres")).toHaveText("+ genres");
  await expect(page.getByTestId("bd-add-tags")).toHaveText("+ tags");
});

test("adds a tag from the hero pill, persists it, and suggests it on reopen", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  await page.getByTestId("bd-add-tags").click();
  const input = page.getByTestId("bd-tags-input");
  await expect(input).toBeVisible();
  await expect(input).toHaveAttribute("placeholder", "+ add tag…");

  await input.fill("Alpine Computing");
  await expectMutation(
    page,
    { method: "POST", url: OVERRIDES_RPC, expectedStatus: 200 },
    async () => input.press("Enter"),
  );
  await expect(page.getByTestId("bd-tags-editor")).toContainText(
    "Alpine Computing",
  );

  // Escape exits edit mode; the static list shows the saved tag + pill.
  await input.press("Escape");
  await expect(input).toHaveCount(0);
  const tagList = page.getByTestId("bd-tag-list");
  await expect(tagList).toContainText("Alpine Computing");
  await expect(tagList).toContainText("+ tags");

  // Survives a full reload (the override is server state, not UI state).
  await gotoReady(page, `/books/${uuid}`);
  await expect(page.getByTestId("bd-tag-list")).toContainText(
    "Alpine Computing",
  );

  // Reopening surfaces the dropdown on focus (already-assigned values are
  // excluded from the pool, so assert the header — it proves the editor
  // reopened with its dropdown wired up).
  await page.getByTestId("bd-add-tags").click();
  await expect(page.getByTestId("bd-tags-suggestions")).toContainText(
    "ADD TAG",
  );
});

test("adds a genre from the hero pill and shows it as an accented chip", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  await page.getByTestId("bd-add-genres").click();
  const input = page.getByTestId("bd-genres-input");
  await expect(input).toHaveAttribute("placeholder", "+ add genre…");

  await input.fill("Field Notes");
  await expectMutation(
    page,
    { method: "POST", url: OVERRIDES_RPC, expectedStatus: 200 },
    async () => input.press("Enter"),
  );

  await input.press("Escape");
  const genreList = page.getByTestId("bd-genre-list");
  await expect(genreList).toContainText("Field Notes");
  await expect(genreList).toContainText("+ genres");

  // A genres override must not write a `subjects` one alongside it.
  const resp = await request.get(`/api/ebooks/${uuid}`);
  expect(resp.status()).toBe(200);
  const book = (await resp.json()) as {
    genres?: string[];
    subjects: string[];
  };
  expect(book.genres).toEqual(["Field Notes"]);
  expect(book.subjects).toEqual([]);
});

test("surfaces a failed tag save by not persisting the chip", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  await page.route(OVERRIDES_RPC, (route) =>
    route.fulfill({
      status: 500,
      contentType: "application/json",
      body: JSON.stringify({ error: "boom" }),
    }),
  );

  await page.getByTestId("bd-add-tags").click();
  const input = page.getByTestId("bd-tags-input");
  await input.fill("Never Saved");
  await expectMutation(
    page,
    { method: "POST", url: OVERRIDES_RPC, expectedStatus: 500 },
    async () => input.press("Enter"),
  );

  // A rejected save resyncs the editor from the canonical book, so the
  // optimistic chip clears in place — no reload required.
  await expect(page.getByTestId("bd-tags-editor")).not.toContainText(
    "Never Saved",
  );

  // And nothing was persisted server-side.
  await page.unroute(OVERRIDES_RPC);
  await gotoReady(page, `/books/${uuid}`);
  await expect(page.getByTestId("bd-tag-list")).not.toContainText(
    "Never Saved",
  );
});
