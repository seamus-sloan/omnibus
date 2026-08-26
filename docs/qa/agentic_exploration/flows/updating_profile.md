# Updating your profile

| | |
|---|---|
| **Weight** | 2% |
| **Owner-only** | your own account only |
| **Surfaces** | web, iOS |
| **Actions** | `profile.update`, `avatar.replace` |

Change your display name or your picture. Low weight because people do it
rarely — but the display name is copied into other places when it is set, so
changing it has reach beyond the account page.

**Only ever change your own account.** Never open another user's account,
never change anyone's permissions, and never delete a user.

## Steps

1. Go to your account page.
2. Change your display name to something clearly yours and clearly new —
   include your actor id and a counter, so a stale copy elsewhere is obvious.
3. Save, and confirm the new name appears in the app's own chrome — wherever
   your name is shown while you are signed in.
4. Occasionally replace your picture with an image from the corpus instead, and
   confirm it appears everywhere your avatar does.
5. Navigate away, come back, and confirm both stuck.
6. Reload the page and confirm again.

## Journal

`profile.update` with the old and new display name. `avatar.replace` with the
source filename. Both are per-user state the audit will check.

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

## Sharp edges

- The display name is **copied** into certain shelf names when it is set, so
  those keep the old name until something updates them. Whether that is a bug
  is a judgement call — journal it as `uncertain` and describe exactly what you
  saw, rather than deciding.
- Your account settings are **not** queued when offline. The iOS agent must not
  attempt a profile change while offline; if a control is available offline and
  appears to succeed, that itself is the finding.
- Password and Kindle-email fields live on this page too. **Do not touch
  either** — changing your password locks you out of the rest of the run.
