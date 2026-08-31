import type { APIRequestContext, Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { TEST_PASSWORD } from "../utils/auth";
import { expectNavVisible, gotoReady } from "../utils/nav";

// The unified Settings sidebar (#1324, #1345) splits the per-user surfaces
// into their own sections: Account holds the password card, Kindle the
// Send-to-Kindle destination address (F4.3), Kobo the wireless-sync devices
// (#927/#1439). The legacy standalone `/account` page redirects into Account
// (mirroring `/logs`) so there is one consistently-chromed entry point.
// The seeded session (global setup) is an authed admin, which is a superset
// of the plain-user surface these sections need.
const ACCOUNT = "/settings?section=account";
const KINDLE = "/settings?section=kindle";
const KOBO = "/settings?section=kobo";

const emailInput = (page: Page) => page.getByTestId("kindle-email-input");
const currentPassword = (page: Page) =>
  page.getByTestId("current-password-input");
const newPassword = (page: Page) => page.getByTestId("new-password-input");
const changeStatus = (page: Page) => page.getByTestId("change-password-status");
const displayNameInput = (page: Page) => page.getByTestId("display-name-input");
const profileStatus = (page: Page) => page.getByTestId("profile-status");

test("renders the account section layout", async ({ page }) => {
  await gotoReady(page, ACCOUNT);

  await expect(page.getByTestId("settings-nav-account")).toHaveAttribute(
    "aria-current",
    "page",
  );
  // Profile card sits at the top of the section.
  await expect(page.getByTestId("account-profile-card")).toBeVisible();
  await expect(displayNameInput(page)).toBeVisible();
  await expect(page.getByTestId("display-name-save")).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 2, name: "Password" }),
  ).toBeVisible();
  await expect(page.getByTestId("account-password-card")).toBeVisible();
  await expect(currentPassword(page)).toBeVisible();
  await expect(newPassword(page)).toBeVisible();
  await expect(page.getByTestId("change-password-submit")).toBeVisible();
  // Hidden-formats card (its behavior is covered by hidden_formats.spec.ts).
  await expect(page.getByTestId("account-hidden-formats-card")).toBeVisible();

  // Kindle and Kobo have their own sections now — not this one.
  await expect(page.getByTestId("account-kindle-card")).toHaveCount(0);
  await expect(page.getByTestId("kobo-devices-card")).toHaveCount(0);

  await expectNavVisible(page);
});

test("renders the Kindle section layout", async ({ page }) => {
  await gotoReady(page, KINDLE);

  await expect(page.getByTestId("settings-nav-kindle")).toHaveAttribute(
    "aria-current",
    "page",
  );
  await expect(
    page.getByRole("heading", { level: 2, name: "Kindle" }),
  ).toBeVisible();
  await expect(page.getByTestId("account-kindle-card")).toBeVisible();
  await expect(emailInput(page)).toBeVisible();
  await expect(page.getByTestId("kindle-email-save")).toBeVisible();
  await expect(page.getByTestId("kindle-email-connected")).toBeAttached();

  await expectNavVisible(page);
});

test("renders the Kobo section layout", async ({ page }) => {
  await gotoReady(page, KOBO);

  await expect(page.getByTestId("settings-nav-kobo")).toHaveAttribute(
    "aria-current",
    "page",
  );
  await expect(page.getByTestId("kobo-devices-card")).toBeVisible();

  await expectNavVisible(page);
});

test("the legacy /account route redirects into the Account section", async ({
  page,
}) => {
  await gotoReady(page, "/account");

  await expect(page).toHaveURL(/\/settings\??$/);
  await expect(page.getByTestId("account-password-card")).toBeVisible();
});

test("saves the Kindle email and shows a success status", async ({ page }) => {
  await gotoReady(page, KINDLE);

  const address = "reader@kindle.com";
  await emailInput(page).fill(address);

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/account/kindle-email",
      expectedBody: { email: address },
      expectedStatus: 200,
    },
    async () => page.getByTestId("kindle-email-save").click(),
  );

  await expect(page.getByTestId("kindle-email-status")).toHaveText(
    "Kindle email saved.",
  );
  await expect(page.getByTestId("kindle-email-status")).toHaveClass(/success/);
});

test("shows an error status when saving the Kindle email fails", async ({
  page,
}) => {
  await gotoReady(page, KINDLE);

  await emailInput(page).fill("reader@kindle.com");

  await page.route("**/api/rpc/account/kindle-email", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 500,
        contentType: "text/plain",
        body: "forced failure",
      });
    }
    return route.continue();
  });

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/account/kindle-email",
      expectedStatus: 500,
    },
    async () => page.getByTestId("kindle-email-save").click(),
  );

  await expect(page.getByTestId("kindle-email-status")).toHaveClass(/error/);
});

// Change-password (#1325). The success test mocks the mutation: the suite
// reuses this admin's credentials across every spec, so an actual password
// change would strand later `ensureLoggedIn` calls. The real DB behavior
// (verify current, validate new, stamp `password_changed_at`) is covered by
// the `omnibus-db` unit tests; the rejection tests below hit the live server
// and are safe because both reject *before* the write.

test("changes the password and clears the form on success", async ({
  page,
}) => {
  await gotoReady(page, ACCOUNT);

  await page.route("**/api/rpc/account/change-password", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: "null",
      });
    }
    return route.continue();
  });

  await currentPassword(page).fill(TEST_PASSWORD);
  await newPassword(page).fill("Brand-New-Valid-9x");

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/account/change-password",
      expectedBody: {
        current_password: TEST_PASSWORD,
        new_password: "Brand-New-Valid-9x",
      },
      expectedStatus: 200,
    },
    async () => page.getByTestId("change-password-submit").click(),
  );

  await expect(changeStatus(page)).toHaveText("Password changed.");
  await expect(changeStatus(page)).toHaveClass(/success/);
  await expect(currentPassword(page)).toHaveValue("");
  await expect(newPassword(page)).toHaveValue("");
});

test("rejects a wrong current password and leaves the form untouched", async ({
  page,
}) => {
  await gotoReady(page, ACCOUNT);

  await currentPassword(page).fill("definitely-not-the-password");
  await newPassword(page).fill("Brand-New-Valid-9x");

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/account/change-password",
      expectedBody: {
        current_password: "definitely-not-the-password",
        new_password: "Brand-New-Valid-9x",
      },
      expectedStatus: 500,
    },
    async () => page.getByTestId("change-password-submit").click(),
  );

  await expect(changeStatus(page)).toHaveClass(/error/);
  await expect(changeStatus(page)).toContainText(
    "current password is incorrect",
  );
  // Inputs keep their values so the user can correct just the current field.
  await expect(currentPassword(page)).toHaveValue(
    "definitely-not-the-password",
  );
});

test("rejects an invalid new password", async ({ page }) => {
  await gotoReady(page, ACCOUNT);

  // Correct current password authorizes; the too-short new password fails
  // policy on the server before any write happens.
  await currentPassword(page).fill(TEST_PASSWORD);
  await newPassword(page).fill("short");

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/account/change-password",
      expectedBody: { current_password: TEST_PASSWORD, new_password: "short" },
      expectedStatus: 500,
    },
    async () => page.getByTestId("change-password-submit").click(),
  );

  await expect(changeStatus(page)).toHaveClass(/error/);
});

// Display name (profile card). The seeded admin is shared across the parallel
// suite and the display name is globally visible — it relabels the wishlist
// shelf and every journal/rating byline — so the success test resets it in a
// `finally`, and the failure test mocks the response rather than writing.

test("saves the display name and shows a success status", async ({ page }) => {
  await gotoReady(page, ACCOUNT);

  try {
    await displayNameInput(page).fill("Seamus");

    await expectMutation(
      page,
      {
        method: "POST",
        url: "/api/rpc/account/profile",
        expectedBody: { display_name: "Seamus" },
        expectedStatus: 200,
      },
      async () => page.getByTestId("display-name-save").click(),
    );

    await expect(profileStatus(page)).toHaveText("Profile saved.");
    await expect(profileStatus(page)).toHaveClass(/success/);

    // The card re-reads `/api/auth/me` after the save, so the user menu shows
    // the new name without a reload.
    await page.getByTestId("user-menu-trigger").click();
    await expect(page.getByTestId("user-menu-panel")).toContainText("Seamus");
  } finally {
    await page.keyboard.press("Escape");
    await displayNameInput(page).fill("");
    await expectMutation(
      page,
      {
        method: "POST",
        url: "/api/rpc/account/profile",
        expectedBody: { display_name: null },
        expectedStatus: 200,
      },
      async () => page.getByTestId("display-name-save").click(),
    );
  }
});

test("shows an error status when saving the display name fails", async ({
  page,
}) => {
  await gotoReady(page, ACCOUNT);

  await displayNameInput(page).fill("Seamus");

  await page.route("**/api/rpc/account/profile", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 500,
        contentType: "application/json",
        body: JSON.stringify({ message: "forced failure" }),
      });
    }
    return route.continue();
  });

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/account/profile",
      expectedStatus: 500,
    },
    async () => page.getByTestId("display-name-save").click(),
  );

  await expect(profileStatus(page)).toHaveClass(/error/);
});

test("shows an error when the avatar upload fails", async ({ page }) => {
  await gotoReady(page, ACCOUNT);

  await page.route("**/api/account/avatar", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 400,
        contentType: "text/plain",
        body: "unsupported image type",
      });
    }
    return route.continue();
  });

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/account/avatar",
      expectedStatus: 400,
    },
    async () =>
      page.getByTestId("avatar-file-input").setInputFiles({
        name: "avatar.png",
        mimeType: "image/png",
        buffer: Buffer.from(
          "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
          "base64",
        ),
      }),
  );

  await expect(page.getByTestId("avatar-status")).toBeVisible();
  await expect(page.getByTestId("avatar-status")).toHaveClass(/error/);
});

// The five goal tests all reset the same three per-user rows, so they have to
// run one at a time — under the suite's `fullyParallel` default they clobber
// each other's state mid-assertion. Scoped to this block rather than the file,
// so the rest of the account surface keeps its parallelism.
test.describe
  .serial("reading goals", () => {
    /**
     * Seed one goal through the REST route, asserting it landed. A silent
     * setup failure — a moved route, an auth regression — would otherwise
     * surface as a confusing assertion about the UI rather than the setup.
     */
    async function seedGoal(
      request: APIRequestContext,
      path: string,
      data: Record<string, unknown>,
    ) {
      const resp = await request.put(path, { data });
      expect(resp.status(), `seeding ${path} failed`).toBe(200);
    }

    /**
     * Reset every goal so each test below starts from the not-set state. The REST
     * writes invalidate this user's stats cache, so the card reads the cleared
     * values immediately rather than after the TTL.
     */
    async function clearAllGoals(request: APIRequestContext) {
      const annual = await request.put("/api/stats/goal", {
        data: { target: null },
      });
      expect(annual.status(), "clearing the reading goal failed").toBe(200);
      for (const kind of ["pages", "minutes"]) {
        const resp = await request.put("/api/stats/goal/daily", {
          data: { kind, target: null },
        });
        expect(resp.status(), `clearing the daily ${kind} goal failed`).toBe(
          200,
        );
      }
    }

    test("the goals card sets all three targets behind one control", async ({
      page,
      request,
    }) => {
      await clearAllGoals(request);
      await gotoReady(page, ACCOUNT);

      const card = page.getByTestId("account-goals-card");
      await expect(card).toBeVisible();
      await expect(
        page.getByRole("heading", { level: 2, name: "Reading goals" }),
      ).toBeVisible();

      // Read mode: every kind states its target or that it has none, and there is
      // exactly one control — the point of consolidating them here.
      for (const testid of ["goal-books", "goal-pages", "goal-minutes"]) {
        await expect(page.getByTestId(`${testid}-value`)).toHaveText("Not set");
      }
      await expect(card.getByRole("button")).toHaveCount(1);
      await expect(page.getByTestId("goals-edit")).toBeVisible();

      await page.getByTestId("goals-edit").click();
      await page.getByTestId("goal-books-input").fill("24");
      await page.getByTestId("goal-pages-input").fill("30");
      await page.getByTestId("goal-minutes-input").fill("45");

      // One Save, three per-kind writes: the annual route and the daily route
      // can't be batched, so the card fans out and reports what landed.
      await expectMutation(
        page,
        {
          method: "POST",
          url: "/api/rpc/stats-goal",
          expectedBody: { update: { target: 24 } },
          expectedStatus: 200,
        },
        async () => page.getByTestId("goals-save").click(),
      );

      await expect(page.getByTestId("goals-status")).toHaveText("Goals saved.");
      await expect(page.getByTestId("goal-books-value")).toHaveText("24 books");
      await expect(page.getByTestId("goal-pages-value")).toHaveText("30 pages");
      await expect(page.getByTestId("goal-minutes-value")).toHaveText(
        "45 minutes",
      );

      // They survive a reload — the card reads all three back off the server,
      // which is what tells an optimistic render apart from a real write.
      await gotoReady(page, ACCOUNT);
      await expect(page.getByTestId("goal-books-value")).toHaveText("24 books");
      await expect(page.getByTestId("goal-pages-value")).toHaveText("30 pages");
      await expect(page.getByTestId("goal-minutes-value")).toHaveText(
        "45 minutes",
      );
    });

    test("an emptied field clears that goal and leaves the others alone", async ({
      page,
      request,
    }) => {
      await clearAllGoals(request);
      await seedGoal(request, "/api/stats/goal", { target: 24 });
      await seedGoal(request, "/api/stats/goal/daily", {
        kind: "pages",
        target: 30,
      });
      await gotoReady(page, ACCOUNT);

      await page.getByTestId("goals-edit").click();
      // Blanking is how a single form drops a goal — there is no Clear per row.
      await page.getByTestId("goal-pages-input").fill("");

      await expectMutation(
        page,
        {
          method: "POST",
          url: "/api/rpc/stats-goal-daily",
          // `target` is `skip_serializing_if = "Option::is_none"`, so a clear
          // travels as the kind alone — same shape the annual clear uses.
          expectedBody: { update: { kind: "pages" } },
          expectedStatus: 200,
        },
        async () => page.getByTestId("goals-save").click(),
      );

      await expect(page.getByTestId("goal-pages-value")).toHaveText("Not set");
      // The untouched annual target is not restated — only changed kinds are
      // written, so another device's value can't be clobbered by a no-op save.
      await expect(page.getByTestId("goal-books-value")).toHaveText("24 books");
    });

    test("an out-of-range target is refused before it costs a round trip", async ({
      page,
      request,
    }) => {
      await clearAllGoals(request);
      await gotoReady(page, ACCOUNT);

      await page.getByTestId("goals-edit").click();
      // 1,500 is a legal day of pages and an impossible day of minutes, so the
      // same number has to be accepted by one field and refused by the other.
      await page.getByTestId("goal-pages-input").fill("1500");
      await page.getByTestId("goal-minutes-input").fill("1500");
      await page.getByTestId("goals-save").click();

      const status = page.getByTestId("goals-status");
      await expect(status).toContainText("Minutes a day");
      await expect(status).toContainText("1440");
      // Nothing was written — the form stays open on the value it rejected.
      await expect(page.getByTestId("goal-pages-input")).toHaveValue("1500");
    });

    test("a failed save names the kind that didn't land", async ({
      page,
      request,
    }) => {
      await clearAllGoals(request);
      await page.route("**/api/rpc/stats-goal-daily", (route) =>
        route.fulfill({ status: 500, contentType: "text/plain", body: "boom" }),
      );

      await gotoReady(page, ACCOUNT);
      await page.getByTestId("goals-edit").click();
      await page.getByTestId("goal-books-input").fill("24");
      await page.getByTestId("goal-pages-input").fill("30");
      await page.getByTestId("goals-save").click();

      // Three goals are three writes: the reader has to know which one to retry
      // rather than being told "something went wrong".
      const status = page.getByTestId("goals-status");
      await expect(status).toContainText("Pages a day");
      await expect(status).toHaveClass(/error/);
      // The annual write did land, and the form stays open on the failure.
      await expect(page.getByTestId("goal-books-input")).toHaveValue("24");
    });

    test("cancel drops the drafts back to what the server confirmed", async ({
      page,
      request,
    }) => {
      await clearAllGoals(request);
      await seedGoal(request, "/api/stats/goal", { target: 24 });
      await gotoReady(page, ACCOUNT);

      await page.getByTestId("goals-edit").click();
      await page.getByTestId("goal-books-input").fill("99");
      await page.getByTestId("goals-cancel").click();

      await expect(page.getByTestId("goal-books-value")).toHaveText("24 books");
      await page.getByTestId("goals-edit").click();
      await expect(page.getByTestId("goal-books-input")).toHaveValue("24");
    });
  });
