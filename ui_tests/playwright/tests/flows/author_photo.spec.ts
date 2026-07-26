import type { APIRequestContext } from "@playwright/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Minimal valid 1x1 PNG. detect_image_format only inspects magic bytes,
// so the body can be tiny — duplicates the constant from the Rust tests.
const TINY_PNG = Buffer.from([
  0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49,
  0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06,
  0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44,
  0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d,
  0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42,
  0x60, 0x82,
]);

// All tests in this file mutate the cached photo for the same author
// ("Ada Lovelace") — run them serially so the afterEach DELETE on one test
// doesn't race the assertions of another.
test.describe.configure({ mode: "serial" });

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

async function fetchAuthorIdByName(
  request: APIRequestContext,
  name: string,
): Promise<number> {
  const resp = await request.get("/api/rpc/ebooks");
  expect(resp.status(), "GET /api/rpc/ebooks failed").toBe(200);
  const body = (await resp.json()) as {
    books: { creators: { name: string; id: number | null }[] }[];
  };
  for (const book of body.books) {
    const match = book.creators.find((c) => c.name === name);
    if (match?.id) return match.id;
  }
  throw new Error(`no indexed author named ${JSON.stringify(name)}`);
}

test.afterEach(async ({ request }) => {
  // Clean up any uploaded photo so reruns start from a known-empty state.
  // Discovery tests rely on the same authors being present, so we must not
  // touch the author row itself — only the cached photo.
  const ada = await fetchAuthorIdByName(request, "Ada Lovelace").catch(
    () => null,
  );
  if (ada !== null) {
    await request.delete(`/api/authors/${ada}/photo`);
  }
});

test("author page shows letter avatar when no photo is uploaded", async ({
  page,
  request,
}) => {
  const id = await fetchAuthorIdByName(request, "Ada Lovelace");
  await gotoReady(page, `/authors/${id}`);

  // Avatar div with the initial visible; no <img.disc-avatar--photo>.
  const letter = page.locator("div.disc-avatar");
  await expect(letter).toBeVisible();
  await expect(letter).toHaveText("A");
  await expect(page.locator("img.disc-avatar--photo")).toHaveCount(0);
});

test("admin upload swaps the letter avatar for the photo", async ({
  page,
  request,
}) => {
  const id = await fetchAuthorIdByName(request, "Ada Lovelace");

  // PUT the photo through the REST endpoint. The seeded test user is the
  // first user registered, so it has is_admin = true.
  const putResp = await request.put(`/api/authors/${id}/photo`, {
    multipart: {
      photo: {
        name: "ada.png",
        mimeType: "image/png",
        buffer: TINY_PNG,
      },
    },
  });
  expect(putResp.status(), "PUT photo should succeed").toBe(204);

  const img = page.locator("img.disc-avatar--photo");
  // Re-navigate until the photo hero resolves. The author page fetches
  // `get_author` client-side, and the `<img>` swaps back to the letter avatar
  // on a single transient photo-GET error (`broken_photo_src` self-heal, only
  // reset on a fresh mount) — under parallel load either can leave the letter
  // showing on the first paint. A reload re-fetches and re-attempts cleanly;
  // this still fails if the photo genuinely never renders.
  await expect(async () => {
    await gotoReady(page, `/authors/${id}`);
    await expect(img).toBeVisible({ timeout: 3_000 });
  }).toPass({ timeout: 20_000, intervals: [500, 1_000, 2_000] });

  await expect(img).toHaveAttribute("src", `/api/authors/${id}/photo`);
  // Letter variant should no longer render in the hero.
  await expect(page.locator("div.disc-avatar")).toHaveCount(0);

  // Sanity-check the image bytes load — fetch directly from the page
  // context so cookies ride along. We can't rely on `waitForResponse` here
  // because the <img> request may complete before we register the listener.
  const status = await page.evaluate(
    async (url: string) => (await fetch(url)).status,
    `/api/authors/${id}/photo`,
  );
  expect(status).toBe(200);
});

test("admin DELETE clears the photo and the letter returns", async ({
  page,
  request,
}) => {
  const id = await fetchAuthorIdByName(request, "Ada Lovelace");

  // Upload, confirm photo, then DELETE and confirm letter is back.
  const putResp = await request.put(`/api/authors/${id}/photo`, {
    multipart: {
      photo: {
        name: "ada.png",
        mimeType: "image/png",
        buffer: TINY_PNG,
      },
    },
  });
  expect(putResp.status()).toBe(204);

  const delResp = await request.delete(`/api/authors/${id}/photo`);
  expect(delResp.status(), "DELETE photo should succeed").toBe(204);

  await gotoReady(page, `/authors/${id}`);
  await expect(page.locator("div.disc-avatar")).toBeVisible();
  await expect(page.locator("img.disc-avatar--photo")).toHaveCount(0);
});
