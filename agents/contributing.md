# Contributing

## Task loop (`TASKS.md`)
Follow this workflow exactly:
- “Before marking an item off here, ask yourself if you have truly completed the task or if you cut corners.”
- “Be sure to commit your changes before starting a new task.”
- “Never commit changes to this file but do update the checkbox once done.”
- Apply version bumps when appropriate per SemVer guidance.

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
- sample CLI output or screenshots for user-visible behavior.

## Versioning (SemVer)
When bumping `Cargo.toml` version, use Semantic Versioning:
- `PATCH`: non-breaking fixes/improvements/docs/refactors.
- `MINOR`: new backward-compatible features.
- `MAJOR`: any breaking change in CLI behavior, flags, output contracts, or storage expectations.

Keep version bumps task-scoped: finish the task, then bump once.

## Documentation hygiene
Update documentation in the same PR as code changes:
- update `AGENTS.md` when top-level navigation or contributor workflow changes,
- add/update files in `agents/` when module behavior, architecture, or workflows change.
- append a concise entry to `CHANGELOG.md` whenever a new version is released.

Prefer small, targeted updates over large rewrites.
