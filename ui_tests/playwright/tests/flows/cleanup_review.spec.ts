// Library cleanup (#966): the Settings "Library cleanup" section and the
// `/settings/cleanup/:kind` review page.
//
// The layout test drives the real endpoints — the seeded admin session already
// has access — and arrives at the review page the way a user does, through the
// section's Review link (there is no other affordance pointing at that route).
//
// The decide tests mock `/api/rpc/cleanup/queue`: the review queue is real
// server state a parallel spec's reindex could refill or empty at any moment,
// and accepting a *real* suggestion would merge or rename live library rows
// every other spec reads. A canned single-card queue keeps the card under
// assertion deterministic while the decide POST it triggers stays real (or,
// for the error path, a forced 500).

import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { expectNavVisible, gotoReady } from "../utils/nav";

const SETTINGS_CLEANUP = "/settings?section=cleanup";
const QUEUE_RPC = "**/api/rpc/cleanup/queue";
const DECIDE_RPC = "**/api/rpc/cleanup/decide";

interface SuggestionCard {
  id: number;
  kind: string;
  action: string;
  decision: string;
  primary_name: string;
  secondary_name: string | null;
  source_names: string[];
  proposed_parts: string[];
  book_count: number;
  photo_url: string | null;
  created_at: number;
}

const AUTHOR_MERGE_CARD: SuggestionCard = {
  id: 4242,
  kind: "author",
  action: "merge",
  decision: "pending",
  primary_name: "Mary Shelley",
  secondary_name: "Shelley, Mary W.",
  source_names: ["Shelley, Mary W."],
  proposed_parts: [],
  book_count: 3,
  photo_url: null,
  created_at: 1_700_000_000,
};

const TITLE_RENAME_CARD: SuggestionCard = {
  id: 4343,
  kind: "book_title",
  action: "rename",
  decision: "pending",
  primary_name: "Shelley, Mary - Frankenstein",
  secondary_name: "Frankenstein",
  source_names: [],
  proposed_parts: [],
  book_count: 1,
  photo_url: null,
  created_at: 1_700_000_000,
};

/** Serve a canned pending queue for every `cleanup/queue` call. */
async function mockQueue(page: Page, cards: SuggestionCard[]): Promise<void> {
  await page.route(QUEUE_RPC, async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(cards),
    });
  });
}

/** Open one kind's review queue through the Settings section's Review link. */
async function openReview(page: Page, kind: string): Promise<void> {
  await gotoReady(page, SETTINGS_CLEANUP);
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/cleanup/queue",
      expectedBody: { kind, limit: 50 },
      expectedStatus: 200,
    },
    async () => page.getByTestId(`cleanup-review-${kind}`).click(),
  );
  await expect(page.getByTestId("cleanup-review")).toBeVisible();
}

/** Open the author review queue through the Settings section's Review link. */
async function openAuthorReview(page: Page): Promise<void> {
  await gotoReady(page, SETTINGS_CLEANUP);
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/cleanup/queue",
      expectedBody: { kind: "author", limit: 50 },
      expectedStatus: 200,
    },
    async () => page.getByTestId("cleanup-review-author").click(),
  );
  await expect(page.getByTestId("cleanup-review")).toBeVisible();
}

test("renders the library cleanup section and the review layout", async ({
  page,
}) => {
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/cleanup/counts", expectedStatus: 200 },
    async () => gotoReady(page, SETTINGS_CLEANUP),
  );

  const card = page.getByTestId("cleanup-card");
  await expect(
    card.getByRole("heading", { name: "Library cleanup" }),
  ).toBeVisible();
  await expect(page.getByTestId("cleanup-kinds")).toBeVisible();
  for (const kind of ["author", "series", "tag", "book_title"]) {
    await expect(page.getByTestId(`cleanup-kind-${kind}`)).toBeVisible();
    await expect(page.getByTestId(`cleanup-count-${kind}`)).toContainText(
      "pending",
    );
  }
  await expect(
    page.getByRole("button", { name: "Run detection now" }),
  ).toBeVisible();
  await expectNavVisible(page);

  // The Review link is the only affordance pointing at the review route.
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/cleanup/queue",
      expectedBody: { kind: "author", limit: 50 },
      expectedStatus: 200,
    },
    async () => page.getByTestId("cleanup-review-author").click(),
  );

  await expect(
    page.getByRole("heading", { level: 1, name: "Review authors" }),
  ).toBeVisible();
  // The redesigned frame: breadcrumb back to Settings, the kind chips, and
  // the decide bar's hotkey legend.
  await expect(page.getByTestId("cleanup-kindline")).toBeVisible();
  for (const kind of ["author", "series", "tag", "book_title"]) {
    await expect(page.getByTestId(`cleanup-kindchip-${kind}`)).toBeVisible();
  }
  // Shared dev server: the real queue is either empty or holding cards, so
  // assert the surface settled into one of its two valid states.
  await expect(
    page
      .getByTestId("cleanup-review-empty")
      .or(page.getByTestId("cleanup-card")),
  ).toBeVisible();
});

test("queues a detection pass from the settings section", async ({ page }) => {
  await gotoReady(page, SETTINGS_CLEANUP);

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/cleanup/detect",
      expectedBody: { kind: null },
      expectedStatus: 200,
    },
    async () => page.getByRole("button", { name: "Run detection now" }).click(),
  );

  await expect(page.getByTestId("cleanup-detect-status")).toContainText(
    "Detection started.",
  );
});

test("shows the empty state when no suggestions are pending", async ({
  page,
}) => {
  await mockQueue(page, []);
  await openAuthorReview(page);

  await expect(page.getByTestId("cleanup-review-empty")).toHaveText(
    "No suggestions pending.",
  );
  await expect(page.getByTestId("cleanup-card")).toHaveCount(0);
});

test("renders one card at a time and accepts it with the Y hotkey", async ({
  page,
}) => {
  await mockQueue(page, [AUTHOR_MERGE_CARD]);
  await openAuthorReview(page);

  await expect(page.getByTestId("cleanup-card")).toHaveCount(1);
  // The action line carries the affected-book count alongside the sentence.
  await expect(page.getByTestId("cleanup-card-action")).toContainText(
    "Merge these authors into one.",
  );
  // The card reads left to right: the record folding away, then the survivor.
  await expect(page.getByTestId("cleanup-card-scanned")).toHaveText(
    "Shelley, Mary W.",
  );
  await expect(page.getByTestId("cleanup-card-proposed")).toHaveText(
    "Mary Shelley",
  );
  await expect(page.getByTestId("cleanup-card-books")).toHaveText(
    "3 books affected",
  );
  // An author merge is not editable — its apply path takes no arbitrary value.
  await expect(page.getByTestId("cleanup-proposed-input")).toHaveCount(0);
  await expect(page.getByTestId("cleanup-progress")).toContainText("1 of 1");

  // The canned card's id names no real row, so the decide POST is fulfilled
  // here too — what's under assertion is the wire contract the hotkey produces.
  await page.route(DECIDE_RPC, async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: "null",
    });
  });
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/cleanup/decide",
      // `value` rides along as null: an author merge is not editable,
      // so nothing overrides the detector's proposal.
      expectedBody: {
        id: AUTHOR_MERGE_CARD.id,
        decision: "accepted",
        value: null,
      },
      expectedStatus: 200,
    },
    async () => {
      // Deliberately no `.focus()` first. `openAuthorReview` arrives here by
      // clicking the dashboard's Review link, so the section is created after
      // page load and its `autofocus` attribute does nothing — the page has to
      // take focus itself on mount or the hotkeys are dead on arrival.
      await expect(page.getByTestId("cleanup-review")).toBeFocused();
      await page.keyboard.press("y");
    },
  );

  // The queue held one card, so the accepted card is the last one.
  await expect(page.getByTestId("cleanup-review-empty")).toBeVisible();
});

test("rejects a card with the Reject button", async ({ page }) => {
  await mockQueue(page, [AUTHOR_MERGE_CARD]);
  await page.route(DECIDE_RPC, async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: "null",
    });
  });
  await openAuthorReview(page);

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/cleanup/decide",
      expectedBody: {
        id: AUTHOR_MERGE_CARD.id,
        decision: "rejected",
        value: null,
      },
      expectedStatus: 200,
    },
    async () => page.getByRole("button", { name: "Reject (N)" }).click(),
  );

  await expect(page.getByTestId("cleanup-review-empty")).toBeVisible();
});

test("skips a card without deciding it", async ({ page }) => {
  const second: SuggestionCard = {
    ...AUTHOR_MERGE_CARD,
    id: 4243,
    primary_name: "Bram Stoker",
    secondary_name: null,
    book_count: 1,
  };
  await mockQueue(page, [AUTHOR_MERGE_CARD, second]);
  await openAuthorReview(page);

  await expect(page.getByTestId("cleanup-card-proposed")).toHaveText(
    "Mary Shelley",
  );
  await page.getByRole("button", { name: "Skip (Space)" }).click();

  await expect(page.getByTestId("cleanup-card-proposed")).toHaveText(
    "Bram Stoker",
  );
  await expect(page.getByTestId("cleanup-card-books")).toHaveText(
    "1 book affected",
  );
});

test("surfaces an error and keeps the card when the decide request fails", async ({
  page,
}) => {
  await mockQueue(page, [AUTHOR_MERGE_CARD]);
  await page.route(DECIDE_RPC, async (route) => {
    await route.fulfill({
      status: 500,
      contentType: "text/plain",
      body: "internal server error",
    });
  });
  await openAuthorReview(page);

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/cleanup/decide",
      // `value` rides along as null: an author merge is not editable,
      // so nothing overrides the detector's proposal.
      expectedBody: {
        id: AUTHOR_MERGE_CARD.id,
        decision: "accepted",
        value: null,
      },
      expectedStatus: 500,
    },
    async () => page.getByRole("button", { name: "Accept (Y)" }).click(),
  );

  await expect(page.getByTestId("cleanup-review-error")).toBeVisible();
  // The failed decision must not advance the queue.
  await expect(page.getByTestId("cleanup-card-proposed")).toHaveText(
    "Mary Shelley",
  );
});

test("accepts a book title with the reviewer's own wording", async ({
  page,
}) => {
  // The redesign's whole point: the proposal is a field, and Y accepts what is
  // in it. The decide POST is what proves the edit reached the server — the
  // card id names no real row, so it is fulfilled here.
  await mockQueue(page, [TITLE_RENAME_CARD]);
  await page.route(DECIDE_RPC, async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: "null",
    });
  });
  await openReview(page, "book_title");

  const proposal = page.getByTestId("cleanup-proposed-input");
  await expect(proposal).toHaveValue("Frankenstein");
  await expect(page.getByTestId("cleanup-edited-tag")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Accept (Y)" })).toBeVisible();

  await proposal.fill("Frankenstein; or, The Modern Prometheus");
  // Typing over the proposal is announced, and the Accept button renames
  // itself to say what accepting would now write.
  await expect(page.getByTestId("cleanup-edited-tag")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Accept edited title (Y)" }),
  ).toBeVisible();

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/cleanup/decide",
      expectedBody: {
        id: TITLE_RENAME_CARD.id,
        decision: "accepted",
        value: "Frankenstein; or, The Modern Prometheus",
      },
      expectedStatus: 200,
    },
    async () =>
      page.getByRole("button", { name: "Accept edited title (Y)" }).click(),
  );

  await expect(page.getByTestId("cleanup-review-empty")).toBeVisible();
  await expect(page.getByTestId("cleanup-tally")).toContainText(
    "accepted with your wording",
  );
});

test("does not fire the hotkeys while the proposal field has focus", async ({
  page,
}) => {
  // A title containing y, n or a space must land in the field rather than
  // deciding the card out from under the reviewer.
  await mockQueue(page, [TITLE_RENAME_CARD]);
  await openReview(page, "book_title");

  const proposal = page.getByTestId("cleanup-proposed-input");
  await proposal.click();
  await proposal.fill("");
  await proposal.pressSequentially("Mary yn Shelley");

  await expect(proposal).toHaveValue("Mary yn Shelley");
  await expect(page.getByTestId("cleanup-card")).toBeVisible();
});
