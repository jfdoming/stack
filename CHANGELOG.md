# Changelog

All notable changes to this repository are documented here. Each version in `Cargo.toml` is treated as a release.

## 0.6.0 - 2026-02-16
- `stack track` now treats inference as the default when `--parent` is omitted in single-branch mode, with interactive parent-selection fallback when inference cannot resolve.

## 0.5.0 - 2026-02-16
- `stack track` now handles missing `--parent` like create/delete option selection: auto-assumes the only viable parent, prompts in TTY mode when multiple parents are available, and errors in non-interactive mode when parent choice is ambiguous.

## 0.4.0 - 2026-02-16
- Renamed `stack unlink` to `stack untrack`.
- `stack untrack` now fully removes the branch from stack metadata and splices tracked children to the removed branch's parent.

## 0.3.1 - 2026-02-16
- `stack track` now mirrors create/delete target selection when branch is omitted: auto-selects the only viable branch, prompts in TTY mode, and errors in non-interactive mode when multiple branches are viable.

## 0.3.0 - 2026-02-16
- Added `stack track` to register relationships for existing local branches.
- Added single-branch and `--all` tracking modes with dry-run and porcelain output.
- Added parent inference using PR base metadata (`gh`) with git-ancestry fallback.
- Added conflict handling for existing parent links, including non-interactive `--force`.
- Added atomic batch parent updates with cycle validation in SQLite writes.

## 0.2.13 - 2026-02-16
- Defaulted inline operation confirmation to `Yes`.
- Auto-selected the only viable branch for create/delete flows and reported the assumption.
- Replaced ambiguous cancellation text after declined operations with a clearer no-op message.
- Standardized contributor docs to Canadian English spellings.

## 0.2.12 - 2026-02-16
- Unified sync and delete confirmations to the inline yes/no toggle style.
- Finalized compact confirmation behavior after prompt UX iterations.

## 0.2.11 - 2026-02-16
- Refined confirmation prompt compactness.

## 0.2.10 - 2026-02-16
- Introduced richer confirmation picker UX for sync and delete flows.

## 0.2.9 - 2026-02-16
- `stack delete` now prompts for tracked branch selection in TTY mode when branch is omitted.
- Non-interactive `stack delete` now requires explicit branch argument.

## 0.2.8 - 2026-02-16
- Clarified help output by separating command-local options from global options.
- Added completion installation examples to `stack completions --help`.

## 0.2.7 - 2026-02-16
- Added `stack delete` command to close/delete upstream PRs, splice stack children, and remove local branches.

## 0.2.6 - 2026-02-16
- Added concise changelog workflow and documentation structure updates.

## 0.2.5 - 2026-02-16
- Rendered cancellation message in red for better visibility.

## 0.2.4 - 2026-02-16
- Added shell completions command and related documentation.

## 0.2.3 - 2026-02-16
- Switched from hardcoded `origin` assumptions to stack base-branch remote resolution.

## 0.2.2 - 2026-02-16
- Added branch creation compare links to `stack create` output.

## 0.2.1 - 2026-02-16
- Added PR compare-link fallbacks in default stack output when PR number is not yet cached.

## 0.2.0 - 2026-02-16
- Initial public milestone with stacked PR CLI/TUI core, sync planner, SQLite persistence, provider abstraction, and install/docs scaffolding.
