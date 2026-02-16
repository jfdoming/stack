# Changelog

Keep entries concise (1-3 bullets each). Newest first.

## Unreleased
- Replaced sync/delete yes-no confirms with richer select-based pickers (clear labels + context lines).
- `stack delete` now uses an interactive tracked-branch picker by default in TTYs (current branch preselected).
- Non-interactive `stack delete` now requires an explicit branch argument instead of assuming current branch.
- Clarified help output by separating command options from global options.
- Added explicit shell completion installation examples to `stack completions --help`.
- Added `stack delete` to close/delete upstream PRs, splice stack children, and delete local branches.
- Added shell completion generation via `stack completions <shell>`.
- Switched stack remote behavior to derive from base branch remote instead of assuming `origin`.
- Added compare-link fallbacks in tree/create output when PR numbers are not cached.
- Hardened interactive Ctrl-C handling for Dialoguer prompts.

## 0.2.5 - 2026-02-16
- Cancellation message now renders in red for better visibility.

## 0.2.4 - 2026-02-16
- Added shell completions command and docs.

## 0.2.3 - 2026-02-16
- Sync/fetch and link generation now respect stack base branch remote.

## 0.2.2 - 2026-02-16
- `stack create` outputs branch creation compare links.

## 0.2.1 - 2026-02-16
- Default tree output now shows compare links for branches without cached PR IDs.
