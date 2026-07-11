import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Journals are public + global per book: every test here lands on the same
// `alpha` book and mutates the shared per-book feed, so run serially (mirrors
// book_detail.spec.ts) — `playwright.config.ts` is otherwise `fullyParallel`,
// which would interleave these and let one test's entries race another's
// assertions. Each test still creates a uniquely-marked entry and deletes it.
test.describe.configure({ mode: "serial" });

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "alpha")!;

type Page = import("@playwright/test").Page;

// The Write surface is a live-formatting `contenteditable` (testid
// `journal-editor`); its markdown is mirrored into a hidden textarea (testid
// `journal-body`) that still carries the `body` signal. Tests type into the
// editor and assert the markdown via the mirror's value.
const editor = (page: Page) => page.getByTestId("journal-editor");
const editorMarkdown = (page: Page) => page.getByTestId("journal-body");

// Publishing rides `create` normally, but if the debounced autosave already
// created the draft row the publish click lands as an `update` instead —
// match either so the assertion is deterministic.
const SAVE_URL = /\/api\/rpc\/journals\/(create|update)/;

/** Open the inline composer and publish `body`, asserting the save POST. */
async function publish(page: Page, body: string) {
  await page.getByTestId("journal-open-composer").click();
  await editor(page).fill(body);
  await expectMutation(
    page,
    { method: "POST", url: SAVE_URL, expectedStatus: 200 },
    async () => page.getByTestId("journal-publish").click(),
  );
}

/**
 * Discard the open composer deterministically: wait for the debounced
 * autosave to land (so the draft row definitely exists), then Cancel and
 * assert the discard DELETE — every mutation stays `expectMutation`-asserted
 * and no draft leaks into later tests' feeds.
 */
async function cancelComposerDiscardingDraft(page: Page) {
  await expect(page.getByTestId("journal-autosaved")).toContainText("Auto-saved", {
    timeout: 10_000,
  });
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/journals/delete", expectedStatus: 200 },
    async () => page.getByRole("button", { name: "Cancel" }).click(),
  );
  await expect(page.getByTestId("journal-composer")).toHaveCount(0);
}

/** Delete the entry whose rendered body contains `marker`. */
async function deleteEntry(page: import("@playwright/test").Page, marker: string) {
  const card = page.getByTestId("journal-entry").filter({ hasText: marker });
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/journals/delete", expectedStatus: 200 },
    async () => card.getByTestId("journal-delete").click(),
  );
  await expect(page.getByTestId("journal-entry").filter({ hasText: marker })).toHaveCount(0);
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

test("renders the journal section layout", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  await expectNavVisible(page);

  // Section heading + shared-log blurb + collapsed composer prompt.
  await expect(page.getByTestId("journal-section")).toBeVisible();
  await expect(page.getByRole("heading", { name: "What readers have written" })).toBeVisible();
  await expect(page.getByTestId("journal-open-composer")).toBeVisible();

  // Expanding the composer reveals the live editor, the spoiler-syntax hint,
  // the formatting toolbar, and the insert-from-highlights control.
  await page.getByTestId("journal-open-composer").click();
  await expect(page.getByTestId("journal-composer")).toBeVisible();
  await expect(editor(page)).toBeVisible();
  await expect(page.getByTestId("journal-spoiler-help")).toBeVisible();
  await expect(page.getByTestId("journal-toolbar")).toBeVisible();
  await expect(
    page.getByTestId("journal-toolbar").getByRole("button", { name: "Bold" }),
  ).toBeVisible();
  await expect(page.getByTestId("journal-insert-highlight")).toBeVisible();
});

// ---------------------------------------------------------------------------
// Action — formatting toolbar wraps the selection (editor-only, no network)
// ---------------------------------------------------------------------------

test("wraps the selected text in markdown when a toolbar button is clicked", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  await page.getByTestId("journal-open-composer").click();
  await editor(page).fill("hello world");

  // Select the whole draft, then Bold → the markdown source is wrapped (read
  // back from the hidden mirror), so the stored format stays plain markdown.
  await editor(page).selectText();
  await page.getByTestId("journal-toolbar").getByRole("button", { name: "Bold" }).click();
  await expect(editorMarkdown(page)).toHaveValue("**hello world**");

  // A line-prefix command (Quote) prefixes the line rather than wrapping.
  await editor(page).fill("a thought");
  await editor(page).selectText();
  await page.getByTestId("journal-toolbar").getByRole("button", { name: "Quote" }).click();
  await expect(editorMarkdown(page)).toHaveValue("> a thought");

  await cancelComposerDiscardingDraft(page);
});

// ---------------------------------------------------------------------------
// Action — insert a saved highlight as a blockquote
// ---------------------------------------------------------------------------

test("inserts a saved highlight into the draft as a blockquote", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);

  // Seed a highlight via the REST API (bearer-authed request fixture); a unique
  // passage keeps the assertion robust against the shared, persistent dev DB.
  const quote = `quotable passage ${Date.now()}`;
  const created = await request.post("/api/highlights", {
    data: {
      book_uuid: uuid,
      epub_cfi_range: "epubcfi(/6/4!/4/2,/1:0,/1:40)",
      color: "amber",
      text: quote,
    },
  });
  expect(created.status(), "seed highlight").toBe(200);

  await gotoReady(page, `/books/${uuid}`);
  await page.getByTestId("journal-open-composer").click();

  // Open the inserter, pick the seeded passage → it lands as a `> ` blockquote
  // with attribution, and the popover closes.
  await page.getByTestId("journal-insert-highlight").click();
  await expect(page.getByTestId("journal-highlights-pop")).toBeVisible();
  await page.getByTestId("journal-highlight-item").filter({ hasText: quote }).click();

  await expect(editorMarkdown(page)).toHaveValue(
    new RegExp(`> ${quote}[\\s\\S]*saved from highlights`),
  );
  await expect(page.getByTestId("journal-highlights-pop")).toHaveCount(0);

  await cancelComposerDiscardingDraft(page);
});

// ---------------------------------------------------------------------------
// Action — create, edit, delete (happy path)
// ---------------------------------------------------------------------------

test("publishes an entry attributed to the current user, edits it, then deletes it", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  const marker = `e2e-journal-${Date.now()}`;
  await publish(page, `First thoughts ${marker}`);

  // The new entry renders with the author's "you" chip (current user owns it).
  const card = page.getByTestId("journal-entry").filter({ hasText: marker });
  await expect(card).toBeVisible();
  await expect(card.getByText("you")).toBeVisible();

  // Owner edit → the body updates in place. Entering edit mode swaps the
  // rendered body for the live editor, so the marker-filtered `card` stops
  // matching (the editor's text isn't the rendered body) — target the single
  // open editor at page level instead.
  await card.getByTestId("journal-edit").click();
  await page.getByTestId("journal-edit-editor").fill(`Revised thoughts ${marker}`);
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/journals/update", expectedStatus: 200 },
    async () => page.getByTestId("journal-edit-save").click(),
  );
  await expect(
    page.getByTestId("journal-entry").filter({ hasText: `Revised thoughts ${marker}` }),
  ).toBeVisible();

  await deleteEntry(page, marker);
});

// ---------------------------------------------------------------------------
// Action — markdown preview + spoiler reveal
// ---------------------------------------------------------------------------

test("renders a markdown preview and blurs spoilers until clicked", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  // Preview renders server-sanitized markdown for the draft.
  await page.getByTestId("journal-open-composer").click();
  await editor(page).fill("**bold preview**");
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/journals/preview", expectedStatus: 200 },
    async () => page.getByTestId("journal-preview-toggle").click(),
  );
  await expect(page.getByTestId("journal-preview").locator("strong")).toHaveText("bold preview");

  // Publish an entry containing a spoiler; it renders blurred until clicked.
  const marker = `e2e-spoiler-${Date.now()}`;
  // Switch back to the Write tab (Preview hides the editor) and compose.
  await page.getByText("Write", { exact: true }).click();
  await editor(page).fill(`reveal ${marker}: ||the secret||`);
  await expectMutation(
    page,
    { method: "POST", url: SAVE_URL, expectedStatus: 200 },
    async () => page.getByTestId("journal-publish").click(),
  );

  const spoiler = page
    .getByTestId("journal-entry")
    .filter({ hasText: marker })
    .locator(".spoiler");
  await expect(spoiler).toHaveText("the secret");
  await expect(spoiler).not.toHaveClass(/revealed/);
  // The wrapper is a native `<button>` so `aria-expanded` reflects state.
  await expect(spoiler).toHaveAttribute("aria-expanded", "false");
  await spoiler.click();
  await expect(spoiler).toHaveClass(/revealed/);
  await expect(spoiler).toHaveAttribute("aria-expanded", "true");

  await deleteEntry(page, marker);
});

// ---------------------------------------------------------------------------
// Action — keyboard users can reveal a spoiler with Enter or Space
// ---------------------------------------------------------------------------

test("reveals a spoiler with Enter and toggles it back with Space", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  // Publish an entry whose only interactive element on the card is the spoiler
  // button, so `focus()` + Enter/Space unambiguously drive it (independent of
  // any surrounding controls).
  const marker = `e2e-spoiler-kb-${Date.now()}`;
  await publish(page, `keyboard ${marker}: ||hidden reveal||`);

  const spoiler = page
    .getByTestId("journal-entry")
    .filter({ hasText: marker })
    .locator(".spoiler");
  await expect(spoiler).toHaveAttribute("aria-expanded", "false");

  // Native `<button>` semantics: Enter fires click → reveals + expands.
  await spoiler.focus();
  await page.keyboard.press("Enter");
  await expect(spoiler).toHaveClass(/revealed/);
  await expect(spoiler).toHaveAttribute("aria-expanded", "true");

  // Space toggles it back to hidden without scrolling the page (the button
  // handles Space natively, so no explicit preventDefault is required).
  await page.keyboard.press("Space");
  await expect(spoiler).not.toHaveClass(/revealed/);
  await expect(spoiler).toHaveAttribute("aria-expanded", "false");

  await deleteEntry(page, marker);
});

// ---------------------------------------------------------------------------
// Action — save a draft, publish it from its card
// ---------------------------------------------------------------------------

test("saves a draft that stays marked Draft until published from its card", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  const marker = `e2e-draft-${Date.now()}`;
  await page.getByTestId("journal-open-composer").click();
  await editor(page).fill(`Draft thoughts ${marker}`);
  await expectMutation(
    page,
    { method: "POST", url: SAVE_URL, expectedStatus: 200 },
    async () => page.getByTestId("journal-save-draft").click(),
  );

  // The composer closes and the entry renders with a Draft chip + a Publish
  // action (drafts are owner-private, so only this user sees it).
  const card = page.getByTestId("journal-entry").filter({ hasText: marker });
  await expect(card).toBeVisible();
  await expect(card.getByTestId("journal-draft-chip")).toBeVisible();

  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/journals/update", expectedStatus: 200 },
    async () => card.getByTestId("journal-publish-draft").click(),
  );
  await expect(card.getByTestId("journal-draft-chip")).toHaveCount(0);
  await expect(card.getByTestId("journal-publish-draft")).toHaveCount(0);

  await deleteEntry(page, marker);
});

// ---------------------------------------------------------------------------
// Action — debounced autosave while typing, discarded on cancel
// ---------------------------------------------------------------------------

test("autosaves the draft while typing and discards it on cancel", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  const marker = `e2e-autosave-${Date.now()}`;
  await page.getByTestId("journal-open-composer").click();

  // Typing arms the 2s debounce; the autosave create fires without any click
  // and the footer surfaces the "Auto-saved" indicator.
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/journals/create", expectedStatus: 200 },
    async () => editor(page).fill(`Autosaved thoughts ${marker}`),
  );
  await expect(page.getByTestId("journal-autosaved")).toContainText("Auto-saved");

  // Cancel discards the autosaved draft row and closes the composer; nothing
  // is left behind in the feed.
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/journals/delete", expectedStatus: 200 },
    async () => page.getByRole("button", { name: "Cancel" }).click(),
  );
  await expect(page.getByTestId("journal-composer")).toHaveCount(0);
  await expect(page.getByTestId("journal-entry").filter({ hasText: marker })).toHaveCount(0);
});

// ---------------------------------------------------------------------------
// Error path — failed publish surfaces an error, composer stays open
// ---------------------------------------------------------------------------

test("surfaces an error and keeps the draft when publishing fails", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  // Fail both save routes — the debounced autosave may have already created
  // the draft row, turning the publish click into an update.
  await page.route("**/api/rpc/journals/create", (route) =>
    route.fulfill({ status: 500, contentType: "text/plain", body: "journal exploded" }),
  );
  await page.route("**/api/rpc/journals/update", (route) =>
    route.fulfill({ status: 500, contentType: "text/plain", body: "journal exploded" }),
  );

  await page.getByTestId("journal-open-composer").click();
  await editor(page).fill("this will fail");
  await expectMutation(
    page,
    { method: "POST", url: SAVE_URL, expectedStatus: 500 },
    async () => page.getByTestId("journal-publish").click(),
  );

  // The composer stays open with the draft intact (read back from the mirror)
  // and an inline error.
  await expect(page.getByTestId("journal-composer")).toBeVisible();
  await expect(editorMarkdown(page)).toHaveValue("this will fail");
  await expect(page.getByTestId("journal-composer").getByRole("alert")).toBeVisible();
});
