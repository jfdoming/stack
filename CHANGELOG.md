# Changelog

All notable changes to this repository are documented here. Each version in `Cargo.toml` is treated as a release.

## Unreleased

## 0.18.11 - 2026-07-22
- Made `stack doctor --fix` detach only actual cycle members, preserving valid descendant links that merely lead into a cycle.

## 0.18.10 - 2026-07-22
- Made cached PR numbers lookup hints only, verified the exact head branch and repository owner, and preserved the base repository identity through close, body-edit, and base-edit mutations.

## 0.18.9 - 2026-07-22
- Moved repository metadata to Git's shared common directory so linked worktrees use one stack database, with safe migration when only a legacy per-worktree database exists.
- Made base discovery prefer existing conventional branches or the current branch when `origin/HEAD` is unavailable, and repair cached bases only when their local ref is missing.
- Made `stack move` reject a tracked parent whose local Git ref is missing before changing stack metadata.

## 0.18.8 - 2026-07-22
- Restricted draft releases to successful same-repository `main` push runs, kept artifact downloads scoped to the triggering run ID, and made the build workflow token read-only.

## 0.18.7 - 2026-07-22
- Passed branch names to batched GitHub GraphQL lookups as raw string fields so leading `@` characters cannot trigger GitHub CLI file substitution.

## 0.18.6 - 2026-07-22
- Removed HTTPS remote credentials, query strings, and fragments from terminal links, browser targets, and generated PR content while retaining the original remote URL for authenticated Git operations.

## 0.18.5 - 2026-07-22
- Made sync discover an advanced remote base before planning, pin and verify the advertised commit across fetch, and restack roots onto that remote state without moving the local base branch.
- Made rewritten-parent restacks use validated reflog fork points, apply parent restacks before descendants, and defer sync metadata updates until the full plan succeeds so obsolete parent commits are not duplicated.

## 0.18.4 - 2026-07-22
- Made merged-stack pruning fail closed unless every present local branch is contained in its fresh merged PR head, with a second safety check immediately before deletion.
- Prevented sync auto-stashes from being restored on the wrong branch; sync now refuses to prune a dirty checked-out branch before stashing and retains the stash if the original branch cannot be restored.

## 0.18.3 - 2026-07-22
- Prevented `stack delete` from deleting the configured base branch or removing its stack metadata.

## 0.18.2 - 2026-07-22
- Prevented option-like branch names from being interpreted as Git flags during branch creation, checkout, rename, deletion, and push operations; create and rename now reject invalid destination names before invoking Git.

## 0.18.1 - 2026-07-22
- Fixed repeated `stack sync` restacks when the base branch has advanced beyond an already-contained merged child commit by persisting the current base SHA.

## 0.18.0 - 2026-07-22
- Added permission-aware placement for new stacks: `stack` now distinguishes Git tracking from canonical/fork repository placement, detects cached GitHub write access, and keeps descendants in their established repository.
- Added `stack config push-target [auto|upstream|fork]` plus matching `stack pr` and `stack push` overrides, with conflict validation that never migrates existing upstreams automatically.
- Added support for custom remote names and distinct fetch/push URLs, actionable upstream-push failures without fork fallback, and clear blocking for impossible fork-only child PRs.

## 0.17.0 - 2026-04-28
- Added `stack split` to split the current branch's committed linear history into tracked stack branches without rewriting commits.
- Added `stack split --top-name <branch>` to rename the current top branch while splitting.
- Added dry-run and porcelain output for split planning, non-porcelain planned stack previews with commits grouped under each branch, confirmation before applying unless `--yes` is passed, plus validation for parent ancestry, duplicate or invalid split points, branch-name conflicts, `HEAD` split points, and merge commits.

## 0.16.1 - 2026-03-13
- `stack move` can now attach untracked local branches under a parent branch, automatically bringing newly linked branches into stack metadata.
- `stack move` now immediately runs sync after updating parent links, so git branch history is restacked to match the new stack relationship.
- In interactive TTY mode, omitting the `stack move` target now opens the picker with the current branch preselected instead of silently auto-choosing it.
- `stack sync` now corrects open PR base branches to match each branch’s tracked parent, alongside the existing managed PR-body refresh.
- `stack pr` and `stack push` now prefer the tracked parent/base branch remote for branches that do not yet have an explicit upstream, keeping new stacked branches in one repo by default.
- `stack sync` now skips impossible cross-repo PR-base corrections with a warning instead of failing `gh pr edit --base`.

## 0.16.0 - 2026-03-13
- Added `stack move [target] --parent <parent>` to reparent a tracked subtree under a new parent while preserving descendants.
- Omitted `stack move` targets now default to the current tracked branch, and interactive runs prompt for any missing target or parent selection.
- Added coverage to reject direct and multi-branch descendant cycles during `stack move`.

## 0.15.0 - 2026-02-25
- Added `stack rename <old> <new>` to rename tracked branches while migrating stack metadata in-place.

## 0.14.2 - 2026-02-25
- Switched batched PR metadata lookup to GraphQL head-alias queries (`gh api graphql`), reducing redundant repository-wide `gh pr list` calls during sync/track/cache refresh flows.
- Fixed sync planning to skip restacks for branches already marked merged when parent/base SHA changes, preventing stale restack prompts after updating the base branch.
- Fixed sync planning to suppress `update_base` when the base branch already contains the merged commit SHA, avoiding repeated no-op base-update prompts.

## 0.14.1 - 2026-02-22
- Fixed a sync planning regression where descendants could be re-restacked when a merged parent branch was tracked but missing locally.
- Added coverage to ensure sync does not plan child restacks when the merged parent ref is absent.

## 0.14.0 - 2026-02-22
- Sync dry-run planning now suppresses no-op `fetch`, `update_base`, and `update_sha` operations when the stack is already current.
- When the entire tracked non-base stack is merged, `stack sync` now prunes merged local branch refs and stack metadata; partially merged stacks are left intact.
- Non-dry-run `stack sync` now exits early with `sync already up to date` when the plan has zero operations.

## 0.13.2 - 2026-02-22
- Hardened stack-managed PR/compare link generation by URL-encoding branch path segments and escaping markdown link labels.
- Sync now prefers fetching `upstream` (when configured) so merged-parent commit SHAs resolve correctly in fork workflows.
- Sync now only advances the local base branch when a direct child PR is merged, and fast-forwards to that exact merge commit SHA rather than the latest base tip.
- Fixed sync rebase fallback after squash merges by anchoring merged-parent child restacks on the parent branch tip, preventing already-merged parent commits from being replayed.
- Sync now treats cached merged branches as merged when fresh PR metadata is unavailable, skipping direct restack/update ops for those branches to avoid conflicting rewrites.
- `stack push` now skips tracked branches marked merged in cached PR metadata, preventing redundant/conflicting pushes of already-merged branches.
- Fixed repeated `stack sync` no-op restack planning after merge-driven rebases by gating merged-parent descendant restacks on ancestry and preserving base sync SHA updates correctly.

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
