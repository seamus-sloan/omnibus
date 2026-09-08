# Flow catalog

Index of the flows an agent is handed, one at a time, by the runner. The
contract every agent reads first is [start.md](../start.md).

| Flow | Weight | Owner-only |
|---|---|---|
| [reading_a_book](reading_a_book.md) | 18% | no |
| [browsing_book_details](browsing_book_details.md) | 17% | no |
| [listening_to_audiobook](listening_to_audiobook.md) | 13% | no |
| [adding_book](adding_book.md) | 10% | — creates ownership |
| [sorting_the_library](sorting_the_library.md) | 7% | no |
| [browsing_authors](browsing_authors.md) | 6% | no |
| [browsing_series](browsing_series.md) | 6% | no |
| [viewing_stats](viewing_stats.md) | 5% | no — own account |
| [searching_the_library](searching_the_library.md) | 5% | no |
| [wishlist](wishlist.md) | 4% | no |
| [creating_a_shelf](creating_a_shelf.md) | 4% | no — web can create but not fill |
| [checking_in_a_book](checking_in_a_book.md) | 3% | removing a copy: yes |
| [updating_profile](updating_profile.md) | 2% | own account |
| [adding_highlight](adding_highlight.md) | 50% of a reading flow | no |
| [resuming_from_another_device](resuming_from_another_device.md) | 25% of a reading flow | no — the runner plays the device |
| [adding_bookmark](adding_bookmark.md) | 50% of a listening flow | no |
| [editing_metadata](editing_metadata.md) | 25% of a details flow | no |
| [adding_journal](adding_journal.md) | 25% of a details flow | no |
| [merging_books](merging_books.md) | 50% of an add-a-book flow | **yes, both books** |
| [deleting_a_book](deleting_a_book.md) | 25% of an add-a-book flow | **yes** |

The thirteen top-level flows sum to 100%; the rest run inside their parent.
Weights here are **suggested defaults** — the runner's configuration is
authoritative, and you never sample anything yourself. You are handed a flow;
you execute it.

Two scenarios are deliberately **not** in this table, because they are not
drawn — the runner hands them to one agent on top of that agent's draw, and a
weight would only unbalance a distribution they never take part in:

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
- **Password and Kindle-email changes.** On the account page beside the
  profile, and off-limits for the reasons `updating_profile.md` gives: a
  changed password locks the agent out of the rest of its run.
