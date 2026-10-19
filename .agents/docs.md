# Documentation Guidelines

Use this file for documentation update rules and scope decisions.

## When To Update Docs
- Update docs in the same PR as behaviour, workflow, or architecture changes.
- If user-facing output/help text changes, update relevant docs in the same task.
- If command flags/subcommands change, update usage examples and workflow notes.
- Maintenance-only tasks and docs-only updates do not require a version bump or changelog entry unless runtime user-facing behaviour changes.

## Where To Update
- `AGENTS.md`: only index/navigation and global guardrails.
- `.agents/contributing.md`: process, commit, versioning, PR expectations.
- `.agents/development.md`: commands, run/test flow, local behaviour notes.
- `.agents/architecture.md`: module responsibilities, data model, behavioural design.
- `README.md`: user-facing quick start and common command examples.
- `CHANGELOG.md`: only when a new release version is created (not for docs-only changes).
  Released version sections are immutable once that version bump is committed/pushed.
  Put subsequent changes under `## Unreleased` until the next release.

## README Scope
- Keep `README.md` focused on the project overview, installation, quick start, and common workflows.
- Do not add a README section for each feature, bug fix, troubleshooting case, recovery procedure, or implementation detail.
- Document detailed behaviour and recovery procedures in `.agents/development.md` or `.agents/architecture.md`, as appropriate. Record release changes in `CHANGELOG.md`.
- Update the README only when its existing overview or common usage guidance needs to change, or when the user explicitly requests an addition.

## Writing Style
- Use Canadian English for user-facing docs.
- Prefer concise, task-scoped updates over broad rewrites.
- Keep examples executable and up to date.
- Avoid duplicating the same detailed guidance across multiple files.

## Consistency Checks
- Ensure command names/flags in docs match the CLI help output.
- Ensure documented behaviour matches tests and implementation.
- If a new `.agents/*.md` file is added or repurposed, update `AGENTS.md` index.
