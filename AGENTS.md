# Repository Guidelines

This file is the high-signal contributor index. Keep it short and move detailed procedures to `.agents/` markdown files.

## Fast Index
- `.agents/contributing.md`: task loop, commit rules, SemVer, PR/doc hygiene.
- `.agents/development.md`: build/test/run commands and local workflow.
- `.agents/architecture.md`: module layout, data model, and sync behaviour.
- `CHANGELOG.md`: release history (one section per version).

## Mandatory Read Order
Read these before making any code or doc change:
1. `TASKS.md`
2. `.agents/contributing.md`
3. `.agents/development.md`

Then read `.agents/architecture.md` for task-relevant codebase context.

## Critical Global Rules
- Execute tasks strictly in `TASKS.md` order, one at a time.
- Re-read `TASKS.md` after each completed task.
- Create exactly one commit per completed `TASKS.md` item.
- Prompt for missing required args/options in interactive mode whenever practical.
- Use Canadian English in user-facing text/docs.
- Docs-only changes do not require a version bump or changelog entry.

## Scope Of `AGENTS.md`
- Keep only global guardrails and doc index/navigation in this file.
- Put command recipes, architecture details, style specifics, and workflow depth in `.agents/*.md`.
- If `AGENTS.md` grows with non-critical detail, move that detail into the proper indexed file.

## Maintenance
When changing behaviour, architecture, or workflows:
1. Update relevant `.agents/*.md` files in the same PR.
2. Update this index only when indexed files are added, removed, or repurposed.
