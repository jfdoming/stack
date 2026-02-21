# Architecture

This project is a Rust CLI/TUI for stacked PR workflows.

## Module map
- `src/main.rs`: process entrypoint and top-level cancellation handling.
- `src/app.rs`: runtime bootstrap (CLI parse, git/db/provider init) and command dispatch.
- `src/args/`: CLI flags/subcommands (`clap`).
- `src/commands/`: per-command execution flows.
- `src/commands/nav.rs`: stack navigation commands (`top`, `bottom`, `up`, `down`) for branch switching.
- `src/core/`: stack graph logic, sync planner, sync executor, plain tree rendering.
- `src/db/`: SQLite schema/migrations and persistence for branches, parent links, sync metadata, PR cache.
- `src/git/`: git command wrapper (branch ops, fetch, replay/rebase, stash, merge-base).
- `src/provider/`: provider abstraction and GitHub implementation via `gh`.
- `src/ui/`: interactive terminal UX helpers and the ratatui `stack` view.
- `src/views/`: JSON-serializable views for porcelain output.
- `src/util/`: shared PR body, URL, and terminal utilities.

## Persistence
- DB location: `.git/stack.db` (repo-scoped).
- Key table: `branches` (single parent relationship, cached PR metadata, sync SHA).
- Integrity: cycle prevention is validated before parent updates.

## Sync behaviour
- Builds a plan (`fetch`, `restack`, metadata updates).
- Prefers `git replay`; falls back to `git rebase --onto` with warning.
- Restores the branch that was checked out before sync once plan execution completes.
- For open PRs discovered during sync, updates the managed stack-flow section in PR bodies while preserving non-managed body text.
- Stops on conflict and warns on stash restore failures.

## Track behaviour
- `stack track` links existing local branches into stack parent relationships.
- When target branch is omitted, selection mirrors create/delete flows (assume single viable branch, prompt on TTY when multiple).
- In single-branch mode, missing `--parent` first tries inference (PR base, then git ancestry), then falls back to the same assumption/prompt pattern when unresolved.
- Git ancestry inference prefers chains that recurse toward the configured base branch.
- Parent inference uses PR base metadata first, then git ancestry heuristics; fork PR lookup retries with `owner:branch` head qualifiers when needed.
- Batch parent updates are validated for cycles and applied atomically.

## Untrack behaviour
- `stack untrack` removes a tracked branch record and splices its children to the removed branch's parent.
- When branch is omitted, target selection mirrors create/delete flows (assume single viable branch, prompt on TTY when multiple).

## PR behaviour
- `stack pr` uses the tracked parent branch as PR base.
- PR creation is skipped when a PR already exists for the current head branch.

## Push behaviour
- `stack push` iterates tracked non-base branches from stack metadata and pushes each branch with `git push --force-with-lease --set-upstream`.
- Branches tracked in metadata but missing locally are skipped with a warning.

## Create behaviour
- `stack create --insert [child]` inserts the new branch between the child's prior parent and the child itself.
- Insert operations update affected open PR managed-body sections to reflect the new parent/child chain.

## Security-relevant behaviour
- Mutating GitHub provider commands fail closed: `gh` non-zero exits during PR create/close are surfaced as errors.
- Optional PR metadata lookups degrade safely with warnings so offline sync/delete workflows can continue.
- Remote URLs derived from git config are sanitized before display to avoid terminal control-character injection.

## Doctor behaviour
- `stack doctor` validates stack metadata integrity and reports repairable issues.
- `stack doctor --fix` can remove missing-branch records, clear invalid base-parent links, break parent-link cycles by clearing implicated parent links, and reset incomplete PR cache fields.
