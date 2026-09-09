# Flow catalog

Index of the flows an agent is handed, one at a time, by the runner. The
contract every agent reads first is [start.md](../start.md).

**Every top-level flow is equally likely to be drawn.** There is no weight:
the run is looking for defects, not modelling how often a reader does
something, and a weighted draw over a handful of distinct flows mostly decided
which rare flows never ran. A subflow **always** runs inside its parent once
the parent is drawn, for the same reason. The sampler reads this table, so the **Runs** cell
is either `on its own` or `inside <parent>`, exactly.

| Flow | Runs | Owner-only |
|---|---|---|
| [reading_a_book](reading_a_book.md) | on its own | no |
| [browsing_book_details](browsing_book_details.md) | on its own | no |
| [listening_to_audiobook](listening_to_audiobook.md) | on its own | no |
| [adding_book](adding_book.md) | on its own | — creates ownership |
| [sorting_the_library](sorting_the_library.md) | on its own | no |
| [browsing_authors](browsing_authors.md) | on its own | no |
| [browsing_series](browsing_series.md) | on its own | no |
| [viewing_stats](viewing_stats.md) | on its own | no — own account |
| [searching_the_library](searching_the_library.md) | on its own | no |
| [wishlist](wishlist.md) | on its own | no |
| [creating_a_shelf](creating_a_shelf.md) | on its own | no — web can create but not fill |
| [checking_in_a_book](checking_in_a_book.md) | on its own | removing a copy: yes |
| [updating_profile](updating_profile.md) | on its own | own account |
| [adding_highlight](adding_highlight.md) | inside reading_a_book | no |
| [resuming_from_another_device](resuming_from_another_device.md) | inside reading_a_book | no — the runner plays the device |
| [adding_bookmark](adding_bookmark.md) | inside listening_to_audiobook | no |
| [editing_metadata](editing_metadata.md) | inside browsing_book_details | no |
| [adding_journal](adding_journal.md) | inside browsing_book_details | no |
| [merging_books](merging_books.md) | inside adding_book | **yes, both books** |
| [deleting_a_book](deleting_a_book.md) | inside adding_book | **yes** |

You never sample anything yourself. You are handed a flow; you execute it.

Two scenarios are deliberately **not** in this table, because they are not
drawn — the runner hands them to one agent on top of that agent's draw:

- the iOS lane's `offline_outbox`, in [ios_lane.md](../ios_lane.md) — the one
  iOS agent always gets it;
- the Kobo scenario, in [kobo_sync.md](../kobo_sync.md) — one web agent gets
  it when the run is started with `--kobo`.

## Not covered, and why

Surfaces no flow reaches. Each is a decision, not an oversight; an agent that
finds itself on one of them has wandered off a flow.

- **Settings, the admin health page, the logs page, and cleanup review.** All
  instance-wide configuration or admin tooling. One edit breaks the run for
  every other agent, so they are on the rails in
  [start.md](../start.md#rails). Admin coverage needs a scratch instance of
  its own, not a flow here.
- **User management** — creating, editing, promoting or deleting users. Same
  reason; and `provision.sh` already owns the accounts.
- **Send to Kindle and Send to Kobo (the download).** They deliver real files
  to real places.
- **Registration.** Every agent is provisioned; a self-registered account
  would own nothing and be audited by nobody.
- **Downloads for offline use on the web.** The "Available offline" section
  and its download rows are rendered by the mobile shell only; the web detail
  page has no download-for-offline control, so there is nothing to drive.
  Offline behaviour is the iOS lane's job.
- **The Android hybrid shell.** No driver exists for it. Its markup is the web
  frontend's, so web flows cover most of what it renders, but its native
  chrome is unexercised.
- **Shelf deletion and membership on the web.** Both exist only on the iOS
  shelf screen; the web can create, select and edit a shelf but never fill or
  delete one, and `creating_a_shelf.md` says which steps are iOS-only.
- **Password and Kindle-email changes.** On the account page beside the
  profile, and off-limits for the reasons `updating_profile.md` gives: a
  changed password locks the agent out of the rest of its run.

## Telling data damage from a defect

The instance is long-lived and carries state earlier runs left behind, some of
it wrong — books whose titles, authors or series were cross-wired by a merge
that lost a side, and rows with no files and no copies. Several flows' fail
criteria fire on that state as readily as on a real fault, so a run can spend
its whole budget re-filing damage.

The test, which an agent worked out in run r-20260908-02:

> **Compare a book's own renderings against each other, not against your idea
> of the truth.** If every surface that names a fact agrees — the eyebrow, the
> block that repeats it, the link that follows it, the table cell that lists it
> — the app is faithfully rendering whatever is stored, and a wrong value is a
> data question for the runner. Only when those surfaces disagree with *one
> another* is it a defect.

The runner tells you in your brief which books are known to be damaged. If you
meet something that looks wrong on one of them, apply the test before reporting
it, and say which way it came out.
