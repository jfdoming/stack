# Changelog

All notable changes to this repository are documented here. Each version in `Cargo.toml` is treated as a release.

## 0.8.16 - 2026-02-16
- Stacked-branch PR URLs now include a `Managed by stack` body section with parent/child branch links, prepended ahead of any user-provided PR body text.

## 0.8.15 - 2026-02-16
- `stack pr` now auto-opens the generated PR URL in a browser after push, with a manual URL fallback message when opener launch fails.

## 0.8.14 - 2026-02-16
- `stack pr` now detects fork branches and builds compare links against `upstream` (when configured), using `owner:branch` head refs for cross-repo PRs.

## 0.8.13 - 2026-02-16
- `stack pr` now blocks self-targeted PR attempts (`base == head`) with a clear user-facing message and porcelain metadata instead of generating broken links.
- Stack tree output now explicitly marks same-base/head branches as `no PR (same base/head)` instead of rendering invalid compare links.

## 0.8.12 - 2026-02-16
- Removed redundant `PR:none` badges from stack output; branches without PRs now use the `no PR` compare link as the sole indicator.

## 0.8.11 - 2026-02-16
- Renamed the stack sync status badge from `SYNC:unsynced` to `SYNC:never` to clarify that no prior `stack sync` SHA has been recorded.

## 0.8.10 - 2026-02-16
- Changed `stack pr` to stop creating PRs directly; it now pushes the branch and prints an open-PR compare link for manual title/body editing in GitHub.
- Hardened gh JSON calls by forcing colourless output (`NO_COLOR=1`, `CLICOLOR=0`) before parsing.

## 0.8.9 - 2026-02-16
- Updated stack tree compare-link label from `open compare` to `no PR` in styled TTY output, while keeping it clickable.

## 0.8.8 - 2026-02-16
- When `stack pr` detects an existing PR, the printed PR hash now renders as a clickable terminal link in styled TTY output.

## 0.8.7 - 2026-02-16
- Fixed inline yes/no redraw anchoring by restoring the original cursor position each toggle, preventing repeated wrapped prompt lines.
- Added overflow fallback for yes/no confirmation prompts: long prompts now use a non-inline selector to avoid wrapped-line redraw artifacts.

## 0.8.6 - 2026-02-16
- Fixed inline yes/no prompt redraw so toggling no longer leaves repeated wrapped lines when prompts exceed terminal width.

## 0.8.5 - 2026-02-16
- Added global `--debug` mode to surface full gh parse/error details; default mode keeps user-facing warnings concise.

## 0.8.4 - 2026-02-16
- `stack pr` now handles existing-PR lookup parse failures gracefully with a user-friendly warning instead of surfacing raw JSON parse errors.

## 0.8.3 - 2026-02-16
- Improved track warning text when PR metadata parsing fails, replacing raw parse errors with clearer fallback messaging.

## 0.8.2 - 2026-02-16
- `stack pr` now supports non-stacked branches by warning and falling back to the repo base branch, while still requiring confirmation unless `--yes` is provided.

## 0.8.1 - 2026-02-16
- Added OSC 8 clickable terminal hyperlinks for PR/compare links in coloured TTY output, replacing raw URL display in those contexts.

## 0.8.0 - 2026-02-16
- `stack pr` now requires confirmation before creating a PR unless `--yes` is provided.

## 0.7.0 - 2026-02-16
- `stack pr` now requires the current branch to be tracked with a tracked parent, ensuring PR base selection always comes from the stack parent relationship.
- `stack pr` now skips creating a new PR when an existing PR is already found for the branch head.

## 0.6.2 - 2026-02-16
- Improved GitHub PR detection for fork-based branches by retrying head lookups with `owner:branch` qualifiers.

## 0.6.1 - 2026-02-16
- When a command auto-assumes a single viable target branch, non-interactive mutating operations now require `--yes` (or an explicit branch) instead of proceeding silently.

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
