# AGENTS.md

This repository's agent guidance is maintained in **one** place — read it
regardless of which agent or tool you are:

- **[CLAUDE.md](CLAUDE.md)** — the index: architecture, rules, skills, quick
  commands, and version control.
- **[.claude/rules/](.claude/rules/)** — numbered rules applied in order; see
  [CLAUDE.md](CLAUDE.md)'s Rules index for the current, authoritative list
  (as of this writing: dev environment, error handling, unit testing,
  Playwright, Rust style, SQL migrations, hydration parity, offline writes,
  content validators, keep-skills-fresh, end-of-session).
- **[.claude/skills/](.claude/skills/)** — task recipes: `add-backend-route`,
  `add-playwright-flow`, `create-github-issue`, `ui-validate`.
- **[docs/feature-map-web.md](docs/feature-map-web.md)** /
  **[docs/feature-map-ios.md](docs/feature-map-ios.md)** — start here to locate
  code: one row per feature, listing the files it spans. They cover features,
  not every file, so fall through to `architecture.md` and then `grep` when a
  row comes up short.
- **[docs/architecture.md](docs/architecture.md)** — the five-crate
  workspace map (per-crate module maps + request-flow diagrams).

This file is intentionally a thin pointer. **Do not duplicate content here** — a
drifting copy is worse than a redirect. Make changes in `CLAUDE.md` / `.claude/`.
