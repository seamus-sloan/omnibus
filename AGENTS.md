# AGENTS.md

This repository's agent guidance is maintained in **one** place — read it
regardless of which agent or tool you are:

- **[CLAUDE.md](CLAUDE.md)** — the index: architecture, rules, skills, quick
  commands, and version control.
- **[.claude/rules/](.claude/rules/)** — numbered rules applied in order: dev
  environment, error handling, unit testing, Playwright, Rust style, and SQL
  migrations.
- **[.claude/skills/](.claude/skills/)** — task recipes: `add-backend-route`,
  `add-playwright-flow`, `ui-validate`.
- **[.claude/architecture.md](.claude/architecture.md)** — the five-crate
  workspace map (per-crate module maps + request-flow diagrams).

This file is intentionally a thin pointer. **Do not duplicate content here** — a
drifting copy is worse than a redirect. Make changes in `CLAUDE.md` / `.claude/`.
