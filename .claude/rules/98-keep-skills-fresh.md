# 98 — Keep skills fresh

Skills under [.claude/skills/](../skills/) are recipes that reference the current shape of the code. When the code changes, the skill can go stale and start handing out wrong advice.

When touching any of the following areas, re-read the matching skill and update it in the **same** change:

| Code area | Skill to re-check |
|---|---|
| `server/src/backend.rs`, `server/src/backend/conditional.rs`, `server/src/main.rs`, `frontend/src/rpc/`, `frontend/src/data.rs`, `db/src/books/`, `db/src/settings/`, `db/src/scanner.rs`, `db/src/ebook.rs`, `db/src/indexer/`, `db/src/library_layout.rs`, `db/src/worker.rs`, `db/migrations/`, `shared/src/lib.rs` | [add-backend-route](../skills/add-backend-route/SKILL.md) |
| `ui_tests/playwright/tests/fixtures/`, `ui_tests/playwright/tests/utils/`, selector conventions | [add-playwright-flow](../skills/add-playwright-flow/SKILL.md) |
| `frontend/src/pages/landing/`, `frontend/assets/atrium.css` (`.lmq` block) | [ui-validate](../skills/ui-validate/SKILL.md) — it drives the landing page as its smoke surface |
| `server/src/backend.rs` (`_health`), `server/src/auth/{boot.rs,gate.rs,handlers.rs}`, `scripts/dev-server-up.sh`, `justfile` (`dev-up`), `.env.example`, `ui_tests/playwright/playwright.config.ts` (baseURL) | [ui-validate](../skills/ui-validate/SKILL.md) |

If a skill no longer has a corresponding code area (the pattern was removed), delete the skill rather than leaving it outdated.

A skill is "stale" if: the file paths it references no longer exist, the function/module names it names have been renamed, or the steps it prescribes would no longer produce a working result, or if the underlying assumptions it relies on have changed.

The same applies to **file:line citations** in the rules, skills, and [docs/style-guide.md](../../docs/style-guide.md): a bare `path/to/file.rs:NN` anchor rots the moment the cited file is edited or split. Prefer function-name anchors (e.g. "`reindex` in `db/src/indexer/ebooks.rs`"); when you touch a file that other docs cite by line, refresh those citations in the same change.
