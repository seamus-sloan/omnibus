# Updating your profile

| | |
|---|---|
| **Runs** | on its own |
| **Owner-only** | your own account only |
| **Surfaces** | web, iOS |
| **Actions** | `profile.update`, `avatar.replace`, `auth.logout`, `auth.login` |

Change your display name or your picture, then sign out and back in. People
do this rarely — but the display name is copied into other places when it is
set, so changing it has reach beyond the account page, and signing back in is
the only time an agent sees the login screen at all.

**Only ever change your own account.** Never open another user's account,
never change anyone's permissions, and never delete a user.

## Steps

1. Go to your account page — Settings → Account on the web (user menu →
   Edit), inside the otherwise off-limits Settings screen; the You tab on
   iOS.
2. Change your display name to a **different plausible person's name** — not
   your actor id, not a counter, not a date. A stale copy elsewhere is just as
   obvious when the chrome still says `Mara Ellison` after you saved
   `Ada Whitfield`, and the name is displayed all over the app and copied into
   your wishlist shelf's name, so it needs to read like a person's.
3. Save, and confirm the new name appears in the app's own chrome — wherever
   your name is shown while you are signed in. The nav trigger shows initials
   or the picture; the full name is inside the opened user menu. On iOS there
   is no toast — the sheet closes and the You header updates.
4. Occasionally replace your picture instead, and confirm it appears everywhere
   your avatar does. On the web the file uploads on selection with no separate
   Save; on iOS the picker is the photo library, which a simulator cannot be
   fed — journal that step `uncertain` there. The corpus is book files, but many author folders carry a
   `cover.jpg` sidecar — use one of those.
5. Navigate away, come back, and confirm both stuck.
6. Reload the page and confirm again.
7. **Sign out**, from the avatar menu. You should land on the login screen,
   and the base URL should now show it too rather than the library. Journal
   `auth.logout`.
8. **Sign in wrongly once**: your username with a password that is off by a
   character. The app must refuse with a clear message and stay on the login
   screen; it must not say *which* of the two was wrong. Journal it as
   `auth.login` with `outcome: refused`. Do this **once** — the login route is
   rate-limited, and hammering it locks the other agents out too.
9. **Sign in correctly.** You should land in the library, and your new display
   name and picture should be the ones shown. Journal `auth.login`. If the
   sign-in fails with your real credentials, stop and tell the runner rather
   than retrying: a locked account is the runner's to fix.

On iOS the equivalent is the **You** tab's sign-out, which returns you to the
connect screen; the wrong-password step is the same there.

## Journal

`profile.update` with the old and new display name. `avatar.replace` with the
source filename. `auth.logout` and `auth.login`, the latter with the outcome
and the message shown. **None of these is checked by the audit**: the profile
is account configuration, deliberately outside the audited per-user state,
and auth is a look. Your journal entries are the record, and the runner reads
them.

## Pass

- The change saves with a confirmation.
- Your name or picture updates in the app chrome without needing a reload.
- It survives navigation and a reload.
- No other user's display appears to change.

## Fail

- The save reports success but the old value comes back.
- The new name appears in one place and the old one persists in another after a
  reload.
- Your picture appears on another user, or theirs on you. **High severity.**
- An avatar upload succeeds but shows a broken image.
- Signing out leaves you signed in — the library still renders at the base
  URL, or your name is still in the chrome. **High severity.**
- A wrong password signs you in, or the refusal says which half was wrong.
- Signing back in shows the old name or picture.

## Sharp edges

- The display name is denormalised into the wishlist shelf's name. Every run
  so far has seen it follow the new name on the same save; a shelf that keeps
  the old name is therefore worth a real observation, journalled `uncertain`
  with exactly what you saw.
- Your account settings are **not** queued when offline. The iOS agent must not
  attempt a profile change while offline; if a control is available offline and
  appears to succeed, that itself is the finding.
- Password and Kindle-email fields live on this page too. **Do not touch
  either** — changing your password locks you out of the rest of the run.
- The Kobo section on this page registers devices. Leave it alone unless you
  were handed [kobo_sync.md](../kobo_sync.md).
- The account page also holds the reading goals and the detail-page
  scroll-stop toggle. Those belong to [viewing_stats.md](viewing_stats.md)
  and [browsing_book_details.md](browsing_book_details.md) respectively; they
  are yours to change, but change them in those flows, not this one.
