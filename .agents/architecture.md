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
- Application bootstrap dispatches repository-independent completion generation before discovering Git or opening stack metadata; all other commands receive the repository-bound application context.

## Persistence
- DB location: `<git-common-dir>/stack.db` (repo-scoped and shared across linked worktrees; normally `.git/stack.db`).
- A legacy per-worktree database atomically claims the shared path only when no shared database exists. Concurrent/conflicting losers are preserved without automatic merging and reported for manual reconciliation.
- Key table: `branches` (single parent relationship, cached PR metadata, sync SHA).
- `repo_meta` schema version 3 stores base-discovery provenance, the push-target policy, cached canonical/fork repository identity, GitHub permission, and detection time.
- Database schema creation, column migration, and version updates run in one immediate SQLite transaction so concurrent worktrees observe one complete migration.
- Base discovery prefers `origin/HEAD` only when both its remote-tracking target and same-named local branch exist, then an existing conventional local base, then the current branch. Provisional current-branch/first-local/default discoveries can yield to a later authoritative remote HEAD; conventional, remote, and legacy bases remain stable while present. Missing cached refs are repaired, with updates conditioned on the exact metadata observed so concurrent linked worktrees cannot overwrite a newer decision.
- Integrity: cycle prevention is validated before parent updates.

## Sync behaviour
- Builds a plan (`fetch`, `restack`, metadata updates) after inspecting the preferred remote's advertised base head and comparing it with the optional local remote-tracking ref.
- Prefers `upstream` as the sync fetch remote when configured; otherwise uses the configured base remote.
- Pins the advertised remote-base commit in the plan and verifies the fetched tracking ref still matches it before applying dependent restacks.
- Prefers `git replay`; falls back to `git rebase --onto` with warning.
- Executes replay using revision ranges (`old_base..branch`) and applies replay-emitted ref updates.
- For restacks with zero commits to replay, uses `git rebase --onto` to fast-forward branch tips to the tracked parent.
- For tracked parent-child restacks, planning captures an immutable `old_base`: the current parent tip when it is still an ancestor, otherwise a validated reflog fork point. Missing rewrite evidence fails closed instead of replaying ambiguous commits.
- Every restack operation also captures the child head reviewed during planning. Replay uses that immutable commit range, rebase uses a temporary recovery branch, and the real child ref is finalized with an expected-old compare-and-swap so another process cannot inject or lose commits between review and mutation.
- Applies restacks parent-first and persists their sync SHAs only after every planned operation succeeds.
- For merged-parent child restacks, execution uses the merged parent branch tip as `old_base` so parent commits are not replayed again over squash-merged base history.
- When a direct child of the base branch is merged and exposes a merge commit SHA, sync fast-forwards the local base branch to that exact merge commit.
- Branches marked merged (from fresh PR metadata or cached merged state) are excluded from direct sync restack/update operations; only descendants are considered for follow-up restacks.
- Merged-parent descendant restacks are gated by ancestry checks so repeated sync runs do not keep emitting no-op restack plans.
- When the base already contains a merged direct child's merge commit, sync records the base's current SHA instead of leaving the prior merge SHA stale.
- Sync planning prunes no-op operations (fetch/update-sha/update-base) when branch sync state is already current.
- Sync execution short-circuits when the computed plan has zero operations.
- Sync branch pruning is all-or-nothing: merged branch refs/metadata are pruned only when the entire tracked non-base stack is merged and every present local tip is contained in its fresh merged PR head. The executor revalidates this proof before each deletion, checks linked-worktree occupancy before and after deletion, restores a ref that raced with checkout, deletes with the validated tip as an expected-old compare-and-swap, and best-effort removes only the exact branch's stale configuration after success.
- Restores the branch that was checked out before sync once plan execution completes. A clean starting branch that is pruned lands on the base branch; a dirty starting branch is never pruned.
- For open PRs discovered during sync, updates the managed stack-flow section in PR bodies while preserving non-managed body text.
- For open PRs discovered during sync, updates both the PR base branch and the managed stack-flow section so GitHub metadata stays aligned with the tracked stack shape.
- PR/push placement is resolved independently of base-branch tracking: existing upstreams are preserved, unpushed descendants inherit the nearest published ancestor, and new roots use the repository push-target policy.
- Canonical/fork topology uses remote fetch and push identities plus a cached GitHub `viewerPermission` lookup. Automatic placement selects upstream for `WRITE`, `MAINTAIN`, or `ADMIN`, otherwise the fork.
- Sync skips PR-base correction when the expected base branch resolves to a different GitHub repository than the PR itself, warning instead of issuing an impossible `gh pr edit --base`.
- Stops fallback rebases on an encoded recovery branch bound to a private, one-time pending ref. After `git rebase --continue`, the next non-dry-run sync atomically consumes that authority while compare-and-swapping the resolved head onto the unchanged target, before planning further work; a matching branch name alone is never trusted. Sync restores an auto-stash by immutable object ID only after verifying that the original branch was restored; otherwise the stash is retained and sync reports an error. Successfully applied auto-stashes remain in the shared stash reflog as recovery entries because reflog-index deletion is unsafe under concurrent linked-worktree updates.
- In interactive TTY mode after successful apply, offers a follow-up push step for tracked non-base branches.

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

## Delete behaviour
- `stack delete` refuses the configured base branch before any provider, Git, or metadata mutation.
- Deleting a tracked non-base branch closes its discovered PR, deletes the local ref, and splices children to the deleted branch's parent.

## Rename behaviour
- `stack rename <old> <new>` renames a tracked branch in local git and updates the existing branch record name in stack metadata.
- Child/parent links are preserved because rename updates the existing branch record identity rather than deleting/reinserting links.
- Rename only updates remotes (push new branch + delete old branch) when the source branch already has an upstream configured.
- If an open PR is detected and upstream deletion is planned, rename warns that deleting the remote branch may close that PR.

## Move behaviour
- `stack move [target] --parent <parent>` reparents a target branch under a new parent while preserving the target subtree beneath it.
- When target and/or parent are local but untracked, move records the new parent link and thereby brings those branches into stack metadata.
- Missing target/parent arguments prompt in TTY mode; when omitted outside TTY, target defaults only when the current local non-base branch is a viable target and parent remains required.
- Move rejects parents inside the target subtree so longer descendant cycles cannot be introduced.
- Move requires the selected parent to have a local Git ref even when stale stack metadata still tracks that name.
- After updating stack metadata, move immediately builds and applies a sync plan so the git branch ancestry is restacked onto the new parent relationship.

## Split behaviour
- `stack split` creates new branch refs at selected commits in the current branch's committed history and wires metadata as `parent -> split branches -> current`.
- Split points are interpreted inclusively: the selected commit becomes the tip of the lower branch, and commits after it appear in the branch above.
- Parent selection uses `--parent`, then the current branch's tracked parent when present, then the repo base branch.
- The current branch remains the top branch by default; `--top-name` renames that top branch while preserving its stack metadata, PR cache, and children.
- Split supports linear histories only and rejects split points at `HEAD`.
- Interactive split prompts default to non-conflicting generated names (`<current>-part-N`) for both lower branches and the top branch.
- Non-porcelain split runs print one planned stack with commits grouped under each branch before applying; non-dry-run splits require confirmation unless `--yes` is passed.
- Dry-run and porcelain output report planned branch creation and metadata links without mutating git or the database.

## PR behaviour
- `stack pr` uses the tracked parent branch as PR base.
- PR creation is skipped when a PR already exists for the current head branch.
- PR creation blocks fork-only child bases that GitHub cannot represent in the canonical repository.
- PR lookup treats cached numbers as hints, verifies the exact head branch and owner, and retains the base repository scope for any later mutation.

## Navigation behaviour
- Stack navigation treats the configured base branch as outside the stack.
- `bottom` resolves to the lowest tracked non-base ancestor; `down` from that root errors instead of switching to base.

## Push behaviour
- `stack push` iterates tracked non-base branches from stack metadata and pushes each branch with `git push --force-with-lease --set-upstream`.
- Push plans resolve and validate repository placement for every targeted stack before the first push; conflicting existing upstreams fail without migration.
- Upstream push failures invalidate cached permission and never retry against the fork.
- Branches marked merged in cached PR state are skipped during push operations.
- Branches tracked in metadata but missing locally are skipped with a warning.

## Create behaviour
- `stack create --insert [child]` inserts the new branch between the child's prior parent and the child itself.
- Insert operations update affected open PR managed-body sections to reflect the new parent/child chain.

## Security-relevant behaviour
- Mutating GitHub provider commands fail closed: `gh` non-zero exits during PR create/close are surfaced as errors.
- Existing PR close/body/base mutations use the repository and verified head identity returned by lookup; bare PR numbers are never mutated outside their repository scope.
- Git branch mutations validate newly created names and terminate option parsing before dynamic branch operands, preventing branch names from selecting destructive Git modes.
- Git fetch, URL lookup, remote inspection, and push commands terminate option parsing before dynamic remote operands, so option-like remote names cannot select command modes.
- Optional PR metadata lookups degrade safely with warnings so offline sync/delete workflows can continue.
- Remote URLs derived from git config are sanitised before display to avoid terminal control-character injection; rendered HTTP(S) links are structurally parsed and stripped of user information, query strings, and fragments before they reach terminal, browser, or PR-body output.
- Batched GitHub GraphQL lookups pass branch names as raw string fields so GitHub CLI cannot apply `@file` or typed-value substitution to ref names.
- The privileged draft-release handoff accepts only successful same-repository `main` push runs and downloads artifacts through the triggering run's ID; pull-request runs cannot cross the privilege boundary even when their head branch is named `main`.
- Generated markdown link labels and branch path segments in stack-managed PR/compare content are escaped/URL-encoded to reduce malformed-link and markdown-injection risks.

## Doctor behaviour
- `stack doctor` validates stack metadata integrity and reports repairable issues.
- `stack doctor --fix` can remove missing-branch records, clear invalid base-parent links, break parent-link cycles by clearing only actual cycle-member links, and reset incomplete PR cache fields; descendants that merely lead into a cycle stay attached.
