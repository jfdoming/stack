# Contributing

## Task loop (`TASKS.md`)
Follow this workflow exactly:
- Work one task at a time. Do not implement multiple open tasks in one pass.
- Treat `TASKS.md` as a queue that can change over time: after finishing a task, continue with the first unchecked item in the latest file order.
- Re-read `TASKS.md` immediately after completing each task.
- “Before marking an item off here, ask yourself if you have truly completed the task or if you cut corners.”
- Commit immediately after completing each task, before starting the next one.
- One commit per completed `TASKS.md` item; do not combine multiple checklist items in one commit.
- “Never commit changes to this file but do update the checkbox once done.”
- Apply version bumps when appropriate per SemVer guidance.
- In interactive mode, prompt for missing required command args/options instead of failing fast where practical.
- Use TDD for behaviour changes: add/update tests first, run to observe failure, then implement and rerun to green.

## Commit style
Use concise Conventional Commit prefixes:
- `feat: ...`
- `fix: ...`
- `refactor: ...`
- `test: ...`
- `docs: ...`

Keep commits incremental and logically scoped.
Use Canadian English in user-facing text and docs.

## Pull requests
PRs should include:
- what changed and why,
- test evidence (command + result),
- sample CLI output or screenshots for user-visible behaviour.
- add or update tests in the same task when runtime behaviour changes (including bug fixes).
- add or update tests when CLI parsing, prompts/defaults, output contracts, or persistence/sync logic changes.
- prefer updating/merging existing tests over adding new ones when coverage can be preserved.
- read `.agents/testing.md` before writing or modifying tests.

## When To Update Docs
- Update docs in the same task when changing behaviour, CLI flags/subcommands/options, workflows, architecture, or user-facing output/help text.
- For docs edits, read `.agents/docs.md` before writing.
- Docs-only changes do not require a changelog entry or version bump.

## Versioning (SemVer)
When bumping `Cargo.toml` version, use Semantic Versioning:
Docs-only changes do not require a version bump.
- Maintenance-only tasks (for example CI/workflow automation or repository housekeeping) do not require a version bump or changelog entry unless runtime user-facing behaviour changes.
- Pre-`1.0.0`:
  - Prefer `PATCH` for non-breaking fixes/improvements/refactors.
  - Use `MINOR` for new features; breaking changes are allowed if required.
- Post-`1.0.0`:
  - Prefer `MINOR` for new backward-compatible features.
  - Use `PATCH` for backward-compatible fixes/refactors.
  - Use `MAJOR` for any breaking CLI/API/schema/output/storage change.

Keep version bumps task-scoped: finish the task, then bump once.
When iterating on the same task after feedback, do not add additional bumps; keep one version bump for that task.

## Documentation hygiene
Update documentation in the same PR as code changes:
- update `AGENTS.md` when top-level navigation or contributor workflow changes,
- add/update files in `.agents/` when module behaviour, architecture, or workflows change.
- append a concise entry to `CHANGELOG.md` whenever a new version is released.
- keep released sections immutable: after a version bump is committed/pushed, do not add/edit bullets under that released version.
- record post-release follow-up changes under `## Unreleased` until the next version bump.
- docs-only changes do not need a changelog entry.
- For iterations on the same task, keep a single changelog entry (update the existing entry instead of adding another).

Prefer small, targeted updates over large rewrites.

## Rust quality checklist
Before finalizing Rust changes, verify:
- model intent with types (prefer explicit structs/enums over ambiguous primitives/flags),
- borrow first and avoid unnecessary clones,
- keep APIs composable and idiomatic (`new`, `from_*`, `as_*`, iterators, traits where useful),
- add contextual errors at boundaries (`anyhow::Context`) and avoid panics for recoverable paths,
- prefer clear `match`/`if let` control flow over deeply nested conditionals,
- keep mutable state and scope minimal,
- run `cargo fmt` and `cargo test` as default quality gates,
- use `cargo clippy` for non-trivial refactors and resolve meaningful lints instead of suppressing by default.
