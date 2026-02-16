# Architecture

This project is a Rust CLI/TUI for stacked PR workflows.

## Module map
- `src/main.rs`: command dispatch and runtime mode selection (TTY vs non-TTY).
- `src/cli/`: CLI flags/subcommands (`clap`).
- `src/core/`: stack graph logic, sync planner, sync executor, plain tree rendering.
- `src/db/`: SQLite schema/migrations and persistence for branches, parent links, sync metadata, PR cache.
- `src/git/`: git command wrapper (branch ops, fetch, replay/rebase, stash, merge-base).
- `src/provider/`: provider abstraction and GitHub implementation via `gh`.
- `src/tui/`: interactive ratatui interface for `stack` visualization.
- `src/output/`: JSON-serializable views for porcelain output.

## Persistence
- DB location: `.git/stack.db` (repo-scoped).
- Key table: `branches` (single parent relationship, cached PR metadata, sync SHA).
- Integrity: cycle prevention is validated before parent updates.

## Sync behaviour
- Builds a plan (`fetch`, `restack`, metadata updates).
- Prefers `git replay`; falls back to `git rebase --onto` with warning.
- Stops on conflict and warns on stash restore failures.

## Track behaviour
- `stack track` links existing local branches into stack parent relationships.
- When target branch is omitted, selection mirrors create/delete flows (assume single viable branch, prompt on TTY when multiple).
- In single-branch mode without `--infer`, missing `--parent` follows the same assumption/prompt pattern.
- Parent inference uses PR base metadata first, then git ancestry heuristics.
- Batch parent updates are validated for cycles and applied atomically.

## Untrack behaviour
- `stack untrack` removes a tracked branch record and splices its children to the removed branch's parent.

## Security-relevant behaviour
- Mutating GitHub provider commands fail closed: `gh` non-zero exits during PR create/close are surfaced as errors.
- Optional PR metadata lookups degrade safely with warnings so offline sync/delete workflows can continue.
- Remote URLs derived from git config are sanitized before display to avoid terminal control-character injection.
