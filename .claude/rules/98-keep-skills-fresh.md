# 98 — Keep skills fresh

Skills under [.claude/skills/](../skills/) are recipes that reference the current shape of the code. When the code changes, the skill can go stale and start handing out wrong advice.

When touching any of the following areas, re-read the matching skill and update it in the **same** change:

| Code area | Skill to re-check |
|---|---|
| `server/src/backend.rs`, `server/src/frontend/`, `server/src/db.rs` | [add-backend-route.md](../skills/add-backend-route.md) |
| `ui_tests/playwright/tests/fixtures/`, `ui_tests/playwright/tests/utils/`, selector conventions | [add-playwright-flow.md](../skills/add-playwright-flow.md) |
| jj workflow changes (new commands, new workspace patterns) | [jj-basics.md](../skills/jj-basics.md), [jj-workspaces.md](../skills/jj-workspaces.md), [jj-advanced.md](../skills/jj-advanced.md) |

If a skill no longer has a corresponding code area (the pattern was removed), delete the skill rather than leaving it outdated.

A skill is "stale" if: the file paths it references no longer exist, the function/module names it names have been renamed, or the steps it prescribes would no longer produce a working result.
