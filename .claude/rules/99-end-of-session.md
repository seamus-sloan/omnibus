# 99 — End of session

Before ending a session where code changed, run this checklist:

1. **Docs sync.** Update the right file for the change:
   - New module or subdirectory → add it to [docs/architecture.md](../../docs/architecture.md) (per-crate module map). If it adds a new top-level crate or top-level concept worth listing in the index, also link it from [CLAUDE.md](../../CLAUDE.md).
   - New dependency in `Cargo.toml` → call it out wherever it's relevant (CLAUDE.md if it affects build commands, the matching rule file if it shifts a convention).
   - New environment variable or configuration key → [.claude/rules/01-dev-environment.md](01-dev-environment.md) and [.env.example](../../.env.example).
   - New or changed convention (error handling, test patterns, etc.) → the matching rule file under [.claude/rules/](.) and the [CLAUDE.md](../../CLAUDE.md) rules index if it's a new file.
   - Notable user-facing or architectural change landing in the next release → add a bullet under `## [Unreleased]` in [CHANGELOG.md](../../CHANGELOG.md).
2. **Skill freshness.** Run [98-keep-skills-fresh.md](98-keep-skills-fresh.md) — verify no skill file got stale.
3. **Nix sync.** If a new system dependency was added, update [flake.nix](../../flake.nix). If the shellHook changed, update [01-dev-environment.md](01-dev-environment.md).
4. **Format & lint.** Run `just lint` (`cargo fmt --check` + clippy across the crate/feature matrix), or `cargo fmt` + `cargo clippy` on the crates you touched.
5. **Unit/integration test coverage.** If any `frontend/`, `backend/`, or `db/` logic changed, ensure a matching test exists per [03-unit-testing.md](03-unit-testing.md), then run `just test` (the full matrix). `just check` runs lint then test in one shot.
6. **Playwright coverage.** If markup contracts changed (roles/labels/testids), update the affected spec under `ui_tests/playwright/tests/flows/` per [04-playwright.md](04-playwright.md).
7. **Line-count cap.** Every file under `CLAUDE.md`, `AGENTS.md`, and `.claude/` should stay under ~200 lines. If any crossed that threshold, split it into multiple topic-scoped files and update the index in [CLAUDE.md](../../CLAUDE.md). (`AGENTS.md` is a thin pointer to `CLAUDE.md`/`.claude/` — keep it that way; don't let content re-accrete there.)
