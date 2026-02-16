# Contributing

## Task loop (`TASKS.md`)
Follow this workflow exactly:
- Work one task at a time. Do not implement multiple open tasks in one pass.
- Re-read `TASKS.md` immediately after completing each task.
- “Before marking an item off here, ask yourself if you have truly completed the task or if you cut corners.”
- Commit immediately after completing each task, before starting the next one.
- One commit per completed `TASKS.md` item; do not combine multiple checklist items in one commit.
- “Never commit changes to this file but do update the checkbox once done.”
- Apply version bumps when appropriate per SemVer guidance.
- In interactive mode, prompt for missing required command args/options instead of failing fast where practical.

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

## When To Update Docs
- Update docs in the same task when changing behaviour, CLI flags/subcommands/options, workflows, architecture, or user-facing output/help text.
- For docs edits, read `.agents/docs.md` before writing.
- Docs-only changes do not require a changelog entry or version bump.

## When To Update Tests
- Add or update tests in the same task when runtime behaviour changes (including bug fixes).
- Add or update tests when CLI parsing, prompts/defaults, output contracts, or persistence/sync logic changes.
- For test edits, read `.agents/testing.md` before writing or modifying tests.

## Versioning (SemVer)
When bumping `Cargo.toml` version, use Semantic Versioning:
For versions below `1.0.0`, breaking changes are permitted in `MINOR` releases.
Docs-only changes do not require a version bump.
- `PATCH`: non-breaking fixes/improvements/docs/refactors.
- `MINOR`: new features; may include breaking changes while `< 1.0.0`.
- `MAJOR`: any breaking change in CLI behaviour, flags, output contracts, or storage expectations once `>= 1.0.0`.

Keep version bumps task-scoped: finish the task, then bump once.

## Documentation hygiene
Update documentation in the same PR as code changes:
- update `AGENTS.md` when top-level navigation or contributor workflow changes,
- add/update files in `.agents/` when module behaviour, architecture, or workflows change.
- append a concise entry to `CHANGELOG.md` whenever a new version is released.
- docs-only changes do not need a changelog entry.

Prefer small, targeted updates over large rewrites.
