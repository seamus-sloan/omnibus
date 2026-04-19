# 98 — Keep skills fresh

Skills under [.claude/skills/](../skills/) are recipes that reference the current shape of the code. When the code changes, the skill can go stale and start handing out wrong advice.

When touching any of the following areas, re-read the matching skill and update it in the **same** change:

| Code area | Skill to re-check |
|---|---|
| `server/src/backend.rs`, `server/src/main.rs`, `frontend/src/rpc.rs`, `frontend/src/db.rs`, `frontend/src/scanner.rs`, `frontend/src/ebook.rs`, `frontend/src/indexer.rs`, `frontend/src/data.rs`, `shared/src/lib.rs` | [add-backend-route](../skills/add-backend-route/SKILL.md) |
| `ui_tests/playwright/tests/fixtures/`, `ui_tests/playwright/tests/utils/`, selector conventions | [add-playwright-flow](../skills/add-playwright-flow/SKILL.md) |
| jj workflow changes (new commands, new workspace patterns) | [jj-basics](../skills/jj-basics/SKILL.md), [jj-workspaces](../skills/jj-workspaces/SKILL.md), [jj-advanced](../skills/jj-advanced/SKILL.md) |

If a skill no longer has a corresponding code area (the pattern was removed), delete the skill rather than leaving it outdated.

A skill is "stale" if: the file paths it references no longer exist, the function/module names it names have been renamed, or the steps it prescribes would no longer produce a working result, or if the underlying assumptions it relies on have changed.
