# Repository Guidelines

This file is the high-signal contributor index. Keep it short and move detailed procedures to `.agents/` markdown files.

## Fast Index
- `.agents/contributing.md`: task loop, commit rules, SemVer, PR/doc hygiene.
- `.agents/development.md`: build/test/run commands and local workflow.
- `.agents/architecture.md`: module layout, data model, and sync behaviour.
- `.agents/docs.md`: documentation update rules, scope, and consistency checks.
- `.agents/testing.md`: test workflow, coverage expectations, and quality bar.
- `CHANGELOG.md`: release history (one section per version).

## Mandatory Read Order
Read these before making any code or doc change:
1. `TASKS.md`
2. `.agents/contributing.md`
3. `.agents/development.md`

Then read `.agents/architecture.md` for task-relevant codebase context.
Read `.agents/docs.md` before making or updating documentation.
Read `.agents/testing.md` before writing or modifying any tests.

## Critical Global Rules
- Execute tasks strictly in `TASKS.md` order, one at a time.
- Re-read `TASKS.md` after each completed task.
- Create exactly one commit per completed `TASKS.md` item.
- Prompt for missing required args/options in interactive mode whenever practical.
- Use Canadian English in user-facing text/docs.
- Docs-only changes do not require a version bump or changelog entry.
- SemVer preference: pre-v1 non-breaking changes should default to `PATCH`; post-v1 non-breaking feature work should default to `MINOR` (see `.agents/contributing.md`).
- Make doc updates in the same task when behaviour, flags, workflows, architecture, or user-facing output changes.
- Add or update tests in the same task when behaviour, control flow, output contracts, or bug fixes change runtime behaviour.

## Scope Of `AGENTS.md`
- Keep only global guardrails and doc index/navigation in this file.
- Put command recipes, architecture details, style specifics, and workflow depth in `.agents/*.md`.
- If `AGENTS.md` grows with non-critical detail, move that detail into the proper indexed file.

## Maintenance
When changing behaviour, architecture, or workflows:
1. Update relevant `.agents/*.md` files in the same PR.
2. Update this index only when indexed files are added, removed, or repurposed.
