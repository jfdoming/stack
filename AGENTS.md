# Repository Guidelines

This file is the contributor index for agent-friendly navigation. Keep it short and move detail into `.agents/` markdown files.

## Fast Index
- `.agents/architecture.md`: module layout, data model, and sync behaviour.
- `.agents/development.md`: build/test/run commands and local workflow.
- `.agents/contributing.md`: commit/PR conventions and doc hygiene.
- `CHANGELOG.md`: repository release history (one section per version).

## How To Use This Index
- Start here for repository orientation.
- Open only the `.agents/*.md` file relevant to your change area.
- If your change spans multiple areas (for example sync logic and CLI UX), update each affected doc.
- Check `TASKS.md` before starting work; it is the active task queue.
- Execute tasks sequentially (one at a time), and commit after each completed task.

## Project Structure (Quick View)
- `src/main.rs`: command entrypoint and dispatch.
- `src/cli/`: `clap` command/flag definitions.
- `src/core/`: stack/sync planning and execution logic.
- `src/db/`: SQLite schema, migrations, persistence.
- `src/git/`: Git command wrappers.
- `src/provider/`: PR provider abstraction + GitHub implementation.
- `src/tui/`: terminal UI.
- `src/output/`: JSON/plain output types.

## Essential Commands
- `cargo build`
- `cargo test`
- `cargo fmt`
- `cargo run -- --help`
- `cargo run -- sync --dry-run`

## Agent Maintenance Policy
Agents should update docs liberally as code evolves.

When changing behaviour, architecture, or workflows:
1. Update the relevant `.agents/*.md` file in the same PR.
2. Update this `AGENTS.md` index if files are added, removed, or repurposed.
3. Prefer concise summaries here; keep detailed rationale and procedures in `.agents/`.

## Style Notes
- Rust style via `rustfmt`; use `snake_case` and `PascalCase` conventions.
- Use Canadian English in user-facing text and documentation.
- Keep commits incremental (`feat:`, `fix:`, `docs:`, etc.).
- Follow SemVer for version bumps, only after atomically completing a given task:
  - Pre-`1.0.0` rule: breaking changes are allowed in `MINOR` bumps.
  - `PATCH` (`x.y.Z`): bug fixes, refactors, docs, and non-breaking UX tweaks.
  - `MINOR` (`x.Y.z`): new features/commands/options; may include breaking changes while `< 1.0.0`.
  - `MAJOR` (`X.y.z`): breaking CLI/API/schema behaviour changes once `>= 1.0.0`.
