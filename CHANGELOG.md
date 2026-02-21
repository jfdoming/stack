# Changelog

All notable changes to this repository are documented here. Each version in `Cargo.toml` is treated as a release.

## Unreleased
- Hardened stack-managed PR/compare link generation by URL-encoding branch path segments and escaping markdown link labels.

## 0.13.1 - 2026-02-21
- Sync now applies replay-emitted branch ref updates and fast-forwards zero-commit restacks via `git rebase --onto`, so inherited parent commits are correctly propagated down the stack.
- Fixed child restacks after parent rewrites to avoid synthetic duplicate empty commits by anchoring replay/rebase on the parent’s pre-sync SHA.
- Stack navigation now excludes the base branch: `bottom` resolves to the root stacked branch and `down` from that root no longer switches to base.

## 0.13.0 - 2026-02-21
- Added `stack push` to push all tracked non-base branches with `git push --force-with-lease --set-upstream`.
- After successful non-dry-run `stack sync` in interactive TTY mode, stack now offers a follow-up push prompt; `--yes` auto-accepts this prompt in TTY mode.
- Fixed sync replay execution by using `git replay --onto <new-base> <old-base>..<branch>` revision ranges.
- Sync now skips no-op restacks when a branch has no commits to replay, avoiding unnecessary replay/rebase fallback churn.
- Added integration coverage for `stack push`, non-fast-forward force-with-lease pushes, and sync non-interactive post-apply push behaviour.

## 0.12.1 - 2026-02-16
- Expanded `stack doctor` diagnostics to report:
  - base branch parent-link corruption (`base_has_parent`),
  - incomplete PR cache fields (`incomplete_pr_cache`).
- `stack doctor --fix` now repairs detected parent-link cycles by clearing cycle-involved parent links.
- `stack doctor --fix` now clears incomplete PR cache metadata so stale partial cache state does not persist.
- Added integration coverage for the new doctor diagnostics and fix paths.

## 0.12.0 - 2026-02-16
- Added `stack create --insert [child]` to insert a new branch between a tracked child branch and its current parent.
- Insert creation now rewires stack metadata in one step (`parent -> new -> child`) and keeps checkout behaviour on the newly created branch.
- Insert creation now refreshes managed stack-flow sections for affected open PR bodies so parent/child links stay current immediately.
- Added integration coverage for metadata relinking, `--insert` target selection, and open-PR body refresh during insert creation.

## 0.11.1 - 2026-02-16
- Refined managed PR-body stack flow formatting:
  - omit leading ellipsis when the base branch is the direct parent,
  - insert an arrow after leading ellipsis (`… →`),
  - render child continuation as `→ …` (with an arrow before trailing ellipsis),
  - omit trailing continuation when the current branch has no child branch,
  - replace `#this PR (this PR)` with `(this PR)`.
- Base-branch links in managed PR-body stack flow now point to the exact merge-base commit (`/commit/<sha>`) instead of the moving base branch tree ref.

## 0.11.0 - 2026-02-16
- `stack track` now refreshes PR cache metadata for newly tracked branches after successful non-dry-run updates, so immediate `stack` output reflects current PR links/states.
- Added integration coverage for track-time PR cache refresh.
- Reapplied shared PR link-target resolution so `stack pr` and `stack` rendering both use consistent per-branch repo/head selection in fork/upstream flows.

## 0.10.11 - 2026-02-16
- Consolidated PR link-target resolution into a shared helper (`src/util/pr_links.rs`) used by both `stack pr` and `stack` summary rendering.
- Fixed `stack` summary PR/compare links to use per-branch repo context (including fork/upstream head refs) instead of a single global repo base.

## 0.10.10 - 2026-02-16
- In `stack` view rendering, base branch entries now always show `no PR (same base/head)` and ignore stale cached PR numbers.
- During `stack sync`, base branch PR cache is explicitly cleared to avoid lingering incorrect PR links.

## 0.10.9 - 2026-02-16
- In sync-managed PR body generation, unresolved parent/child branches now always link to branch paths (`/tree/...`) instead of reusing stale cached PR numbers.
- Excluded the base branch from sync PR metadata association to avoid accidental PR linkage on branch names like `main`/`master`.
- Hardened cached PR fallback parsing when `gh pr view` unexpectedly returns list-shaped JSON.

## 0.10.8 - 2026-02-16
- Fixed managed PR-body link targeting to prefer each detected PR’s own URL/repo, preventing cross-repo link mismatches in fork/upstream workflows.
- Fixed batch PR metadata matching to prefer the branch’s remote owner, avoiding incorrect PR association for common branch names like `main`/`master`.

## 0.10.7 - 2026-02-16
- Fixed GitHub PR detection for fork/upstream workflows by expanding metadata lookups across explicit remote repo scopes (including `upstream`) instead of relying only on default GH repo context.

## 0.10.6 - 2026-02-16
- Updated `stack track` git-ancestry inference to recurse toward the configured base branch when possible, instead of only selecting the nearest local ancestor.
- Added integration coverage to verify recursive inference picks the base branch for deep ancestry chains.

## 0.10.5 - 2026-02-16
- Removed `stack pr` confirmation prompts; PR link open flow now proceeds immediately (including in non-interactive mode).
- Updated integration coverage to confirm `stack pr` succeeds without `--yes` for both stacked and non-stacked branches.

## 0.10.4 - 2026-02-16
- Added a shared branch PR-body helper in `src/util/pr_body.rs` and wired both:
  - `stack pr` default body generation, and
  - `stack` compare-link body generation
  to use the same source, preventing format drift.
- Updated manual PR fallback output so styled TTY mode shows exactly `open PR manually` as clickable OSC 8 text.

## 0.10.3 - 2026-02-16
- Optimized `stack sync` PR metadata refresh by batching GitHub PR list lookups instead of running one `gh` metadata query per branch.
- Added sync integration coverage to assert batched PR metadata lookup usage.
- Refined manual PR fallback link rendering so output now reads `open PR manually: <url>` (clickable in styled TTY mode) without duplicated label text.

## 0.10.2 - 2026-02-16
- Updated `stack pr` manual-open fallback output:
  - styled TTY mode now prints an OSC 8 clickable `open PR manually` link,
  - plain output prints the full URL without truncation.
- Added tests for clickable and plain fallback link formatting.

## 0.10.1 - 2026-02-16
- Fixed stack summary compare-link body text to use Unicode arrows (`→`) instead of ASCII arrows (`->`) in generated Stack Flow descriptions.

## 0.10.0 - 2026-02-16
- `stack sync` now refreshes managed stack-flow PR body sections for existing open PRs.
- Sync preserves user-authored PR body content outside `<!-- stack:managed:start -->` / `<!-- stack:managed:end -->` markers while replacing or adding the managed block.
- Added sync integration coverage to verify `gh pr edit` is called with managed marker content during sync.

## 0.9.1 - 2026-02-16
- Added managed PR body boundary markers to generated `stack pr` descriptions:
  - `<!-- stack:managed:start -->`
  - `<!-- stack:managed:end -->`
- Kept the existing compact stack-flow chain inside those markers and continued appending user-provided body text below the managed block.
- Added tests to validate marker presence in both composed PR body text and generated open-PR URL query parameters.

## 0.9.0 - 2026-02-16
- Added stack navigation commands:
  - `stack top` to jump to the top-most descendant in the current stack path.
  - `stack bottom` to jump to the root ancestor in the current stack path.
  - `stack up` to switch to a direct child branch.
  - `stack down` to switch to the direct parent branch.
- In TTY mode, `stack up` and `stack top` now prompt for child selection when multiple tracked children exist; non-interactive mode reports an ambiguity error.
- Added integration coverage for up/down, top/bottom, and multi-child ambiguity handling.

## 0.8.25 - 2026-02-16
- `stack sync` now restores the branch that was checked out before the sync run, even when restack operations switch branch context.
- Added integration coverage to ensure post-sync branch context returns to the original branch.

## 0.8.24 - 2026-02-16
- Refactored CLI execution into focused command modules and reduced `src/main.rs` to bootstrap/dispatch orchestration.
- Split core behaviour into dedicated `parents`, `render`, and `sync` modules while preserving existing runtime behaviour.
- Reorganized presentation and interaction layering:
  - moved command-agnostic terminal interaction/picker helpers into `src/ui/`,
  - renamed `src/cli` to `src/args`,
  - renamed `src/output` to `src/views`,
  - moved ratatui stack UI under `src/ui/tui.rs`.
- Added `AppContext::build()` bootstrapping in `main` to centralize startup wiring.
- Hardened integration test stability by disabling colourized stderr in harness defaults and adding a browser-open mock env path for test runs.

## 0.8.23 - 2026-02-16
- `stack untrack main` now succeeds as a no-op whether passed explicitly or reached by default when no tracked non-base branches exist.

## 0.8.22 - 2026-02-16
- Suppressed the redundant non-stacked warning when `stack pr` already exits for self-targeted base/head branch PRs.
- Added integration coverage to assert this warning does not appear for the base-branch self-target case.

## 0.8.21 - 2026-02-16
- `stack create` now checks out the newly created branch immediately after creation.

## 0.8.20 - 2026-02-16
- `stack create` no longer emits an immediate compare link (`create_url`), since newly created branches have zero diff by default.

## 0.8.19 - 2026-02-16
- Refined autogenerated PR description text to a shorter, cleaner `Stack Flow` format while retaining parent/child linkage for stacked branches.

## 0.8.18 - 2026-02-16
- `stack pr` now always includes a managed PR description body in generated URLs (base/head links by default, plus parent/child links for stacked branches), with user body text appended below.

## 0.8.17 - 2026-02-16
- Updated missing-PR labels to bracketed form (`[no PR]`), including same-base/head fallback text.

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
