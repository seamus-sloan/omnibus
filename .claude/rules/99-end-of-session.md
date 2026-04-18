# 99 — End of session

Before ending a session where code changed, run this checklist:

1. **Docs sync.** Update [CLAUDE.md](../../CLAUDE.md) or the relevant rule file if any of these happened:
   - New module or subdirectory
   - New dependency in `Cargo.toml`
   - New environment variable or configuration key
   - New or changed convention (error handling, test patterns, etc.)
2. **Skill freshness.** Run [98-keep-skills-fresh.md](98-keep-skills-fresh.md) — verify no skill file got stale.
3. **Nix sync.** If a new system dependency was added, update [flake.nix](../../flake.nix). If the shellHook changed, update [01-dev-environment.md](01-dev-environment.md).
4. **Format & lint.** Run `cargo fmt` and `cargo clippy` on anything touched in the server/mobile crates.
5. **Unit/integration test coverage.** If any `frontend/`, `backend/`, or `db/` logic changed, ensure a matching test exists per [03-unit-testing.md](03-unit-testing.md).
6. **Playwright coverage.** If markup contracts changed (roles/labels/testids), update the affected spec under `ui_tests/playwright/tests/flows/` per [04-playwright.md](04-playwright.md).
7. **Line-count cap.** Every file under `CLAUDE.md` and `.claude/` should stay under ~200 lines. If any crossed that threshold, split it into multiple topic-scoped files and update the index in [CLAUDE.md](../../CLAUDE.md).
