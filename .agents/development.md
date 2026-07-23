# Development Workflow

## Core commands
- `cargo build`: compile the binary.
- `cargo test`: run unit tests.
- `cargo fmt`: apply rustfmt formatting.
- `cargo run -- --help`: top-level command help.
- `cargo run -- top`: switch to the top-most descendant in the current stack path.
- `cargo run -- bottom`: switch to the root stacked ancestor branch (base branch excluded).
- `cargo run -- up`: switch to a direct child branch.
- `cargo run -- down`: switch to the direct parent branch.
- `cargo run -- sync --dry-run`: preview sync plan without execution.
- `cargo run -- create --insert feat/child --name feat/mid`: insert a new branch before a tracked child.
- `cargo run -- track feat/branch --parent main`: track an existing local branch under a parent branch.
- `cargo run -- track --all --dry-run`: preview inferred relationships for all local non-base branches.
- `cargo run -- untrack feat/branch`: remove a tracked branch record and splice children to its former parent.
- `cargo run -- rename feat/old feat/new`: rename a tracked branch and migrate stack metadata.
- `cargo run -- move feat/child --parent feat/new-parent`: move a tracked branch and its descendants under a new parent branch.
- `cargo run -- split --at <commit> --name feat/part-1 --top-name feat/top`: split the current branch's committed linear history into lower tracked stack branches without rewriting commits.
- `cargo run -- completions zsh`: print shell completion script (works for `bash`, `zsh`, `fish`, `elvish`, `powershell`).
- `cargo run -- --yes delete <branch>`: close/delete PR, splice branch from stack, and remove local branch.
- `cargo run -- --debug pr --yes`: include detailed gh parse/debug error output for PR checks.
- `cargo run -- --debug sync --dry-run`: include sync timing metrics in debug output.
- `cargo run -- push`: push all tracked non-base branches with `--force-with-lease`.
- `cargo run -- config push-target [auto|upstream|fork]`: view or set repository-level placement for new stacks.
- `cargo run -- push --push-target upstream`: override placement for unpushed branches while validating existing upstreams.

## CI
- GitHub Actions workflow `.github/workflows/build.yaml` runs tests unconditionally (pull requests and `main` pushes).
- In `build.yaml`, non-release compile (`cargo build --locked --verbose`) runs on pull requests.
- On `main` pushes, `build.yaml` skips non-release compile and only packages release binaries for artifact publishing.
- GitHub Actions workflow `.github/workflows/draft-release.yaml` runs after successful `CI Build` on `main` and creates a draft GitHub release/tag only when a release does not already exist for the current `Cargo.toml` version.
- Workflow `uses:` action refs are pinned to full commit SHAs for supply-chain hardening; update those pins deliberately when bumping action versions.
- Build workflow packages release executables for Linux (`x86_64-unknown-linux-gnu`), macOS (`x86_64-apple-darwin`, `aarch64-apple-darwin`), and Windows (`x86_64-pc-windows-msvc`) on `main` pushes.
- Draft release workflow reuses those packaged build artifacts and attaches them to the draft release for the current version tag when no release exists yet.

## Install from source
- `./scripts/install.sh`: build release binary and install to `~/.local/bin/stack`.
- `./scripts/install.sh --write-shell-config`: also append PATH update to shell config.
- `STACK_INSTALL_PREFIX=/custom/prefix ./scripts/install.sh`: install under a custom prefix.

## Local behaviour notes
- `stack` without args prints a one-shot tree visualization by default.
- `stack --interactive` opens the fullscreen TUI.
- `stack up`/`stack top` prompt for child selection in TTY mode when the current branch has multiple tracked children; non-interactive mode returns an ambiguity error.
- Base branch is excluded from stack navigation (`up`, `down`, `top`, `bottom`); run navigation commands from tracked non-base branches.
- `stack create` switches to the newly created branch and does not print an immediate compare URL because the new branch initially has no diff.
- `stack create --insert [child]` inserts a new branch between the child's current parent and that child, updates stack metadata links, and refreshes managed sections for affected open PR bodies.
- Branch creation and rename reject invalid destination names before invoking Git; dynamic branch operands are passed after an option terminator so malformed legacy refs cannot be interpreted as Git flags.
- Non-interactive contexts fall back to plain text (or JSON with `--porcelain`).
- `stack sync` supports staged application; use `--yes` to auto-confirm.
- `stack doctor --fix` also repairs detected parent-link cycles, clears invalid base-parent links, and resets incomplete PR cache fields.
- After `stack sync` applies operations, it restores the branch that was checked out before the sync run started. When a clean checked-out merged branch is itself pruned, sync leaves the repository on the base branch.
- Sync refuses to prune the checked-out branch while its worktree is dirty. Auto-stashes are restored only after the original branch is re-established; otherwise the stash is retained and sync fails with its stash reference.
- During `stack sync`, open PR bodies are refreshed to keep the managed stack-flow section current; user-written text outside managed markers is preserved.
- During `stack sync`, open PR base branches are corrected to match each branch’s tracked parent (or the repo base branch when no tracked parent exists).
- Repository push placement is independent of the base branch's Git tracking remote. New stack roots use the configured `auto`, `upstream`, or `fork` policy; descendants inherit their nearest published ancestor's repository.
- `auto` performs one GitHub topology/permission lookup when placement is first needed, persists the policy in `.git/stack.db`, and caches successful detection for 24 hours. Interactive first pushes prompt; non-interactive/`--yes` pushes persist automatic detection.
- `stack pr --push-target` and `stack push --push-target` override placement for unpushed branches and fail before pushing when an existing upstream conflicts.
- Remote resolution compares fetch and push URLs separately and supports custom remote names. Rejected upstream pushes stop without a fork fallback.
- A fork-hosted root branch can target the canonical base branch, but `stack pr` blocks a child whose parent exists only in the fork because GitHub cannot use that cross-repository base.
- During `stack sync`, PR base updates are skipped with a warning when the target base branch lives in a different repo from the PR and GitHub cannot accept the cross-repo base.
- After non-dry-run `stack sync` in interactive TTY mode, stack offers a follow-up prompt to run `stack push`; `--yes` auto-accepts that prompt in TTY mode.
- During restack execution, `stack sync` uses `git replay --onto <new-base> <old-base>..<branch>` and applies replay-emitted ref updates.
- When a restack target has zero commits beyond the computed merge-base, sync uses `git rebase --onto` to fast-forward the branch onto its parent.
- Sync queries the preferred remote's advertised base head without mutating refs during planning; when it differs locally, the plan fetches and restacks roots onto that pinned commit in one run.
- For child restacks onto a tracked parent branch, sync uses the current parent tip when it remains in the child history, otherwise a validated reflog fork point. If rewrite evidence is unavailable, sync refuses the ambiguous restack.
- Parent restacks run before descendants, and sync SHAs are persisted only after the complete plan succeeds.
- For child restacks after a merged parent PR (including squash merges), sync anchors replay/rebase `old-base` to the merged parent branch tip so parent commits are dropped and only child commits are replayed.
- In fork workflows, `stack sync` fetches `upstream` when present (instead of `origin`) so merged-parent commit SHAs can be resolved locally before replay/rebase.
- `stack sync` only advances the local base branch when a direct child PR is marked merged and includes a merge commit SHA; the base branch is fast-forwarded to that exact merge commit (not beyond later base-branch commits).
- If a branch is known merged (fresh PR metadata or cached merged state), sync skips direct mutation ops for that branch and only processes its descendants.
- Sync no longer re-plans redundant restacks on repeated runs once descendants already contain the merged-parent target commit.
- When the base branch already contains a merged direct child's merge commit, sync persists the current advanced base SHA so later runs do not re-plan descendant restacks.
- When no merge/restack/metadata updates are needed, `stack sync --dry-run` now emits an empty operation list (no no-op fetch/update entries).
- When a non-dry-run sync plan is empty, `stack sync` exits early with `sync already up to date` and skips apply bookkeeping.
- When every tracked non-base branch in the stack is merged, `stack sync` prunes leaf-first only when every present local tip is contained in fresh merged-PR `headRefOid` metadata; missing local refs can still have their metadata pruned.
- Sync batches GitHub PR metadata lookups to reduce per-branch `gh` round trips on larger stacks.
- PR metadata lookup now checks both default GH context and known remote repo scopes (including `upstream`) to avoid missing PRs in fork workflows.
- Batched PR metadata lookup binds branch-name GraphQL variables with GitHub CLI raw string fields, preserving leading `@` characters without file substitution.
- `stack track` records relationships for existing local branches; it can infer parents from PR base metadata and git ancestry.
- After non-dry-run `stack track`, PR cache metadata is refreshed for newly tracked branches so `stack` view immediately reflects current PR links/states.
- In single-branch track mode, parent inference is attempted by default when `--parent` is omitted.
- Git-ancestry inference now recursively walks parent candidates until the configured base branch when possible.
- Omitting `stack track <branch>` follows create/delete selection behaviour: assume when only one viable branch exists, otherwise prompt in TTY mode.
- In non-interactive mode, if track auto-assumes a single viable target branch and would mutate state, pass `--yes` or an explicit target branch.
- If default inference cannot resolve a parent, single-branch track falls back to the same assumption/prompt flow for parent selection.
- Omitting `stack untrack <branch>` follows the same assumption/prompt flow as delete.
- In non-interactive mode, if untrack auto-assumes a single viable target branch, pass `--yes` or an explicit target branch.
- `stack untrack main` is allowed as a no-op and reports that the base branch remains the stack root.
- `stack delete` rejects the configured base branch; only tracked non-base branches can be deleted.
- `stack rename <old> <new>` renames tracked metadata and the local branch in one command.
- `stack move [target] --parent <parent>` reparents a tracked subtree without changing child links below the moved target.
- `stack move` can also adopt untracked local branches into the stack by attaching them under a parent branch, tracking any newly linked branches as needed.
- Omitting `stack move` target preselects the current local non-base branch in the TTY picker; in non-interactive mode it defaults only when the current branch is a viable target.
- After `stack move` updates parent links, it immediately runs the sync planner/executor so branch history is restacked to match the new stack shape.
- `stack split` turns the current branch's committed linear history into a tracked stack without rewriting commits; each selected `--at` commit becomes the tip of a new lower branch, later commits remain above it, and the current branch remains checked out as the top branch.
- `stack split --top-name <branch>` renames the current branch while keeping it as the top branch; interactive runs prompt for the top branch name with the next non-conflicting `<current>-part-N` name as the default.
- `stack split --parent <branch>` overrides the branch below the first split branch; otherwise split uses the current tracked parent or the repo base branch.
- Before applying, non-porcelain `stack split` prints one planned stack with commits grouped under each branch; non-dry-run splits require confirmation unless `--yes` is passed.
- `stack split --dry-run --porcelain` reports planned branch refs and parent links without mutating git or stack metadata.
- Rename only pushes/deletes remote refs when the source branch already has an upstream configured.
- If rename will delete an upstream branch with an open PR, stack warns that deleting the branch may close that PR.
- Omitting `stack completions <shell>` prompts for shell selection in TTY mode.
- On stacked branches, `stack pr` uses the tracked parent as PR base and skips opening when an existing PR already matches the branch head.
- `stack pr` pushes and auto-opens the PR URL immediately (no confirmation prompt).
- `stack push` pushes all tracked non-base branches and uses `--force-with-lease` for each branch push.
- `stack push` skips branches marked as merged in PR cache metadata.
- If browser auto-open fails, `stack pr` prints a manual fallback link; styled TTY output uses OSC 8 clickable text instead of truncating the URL.
- On non-stacked branches, `stack pr` warns and uses the repo base branch as PR base.
- `stack pr` blocks self-targeted PRs (`base == head`) with a clear message instead of generating a broken compare link.
- When `stack pr` exits early for self-targeted PRs (`base == head`), it suppresses the redundant non-stacked warning.
- For fork branches, `stack pr` builds compare links against `upstream` (when configured) and uses `owner:branch` head refs.
- Generated `stack pr` URLs always include a compact `Stack Flow` PR body section wrapped in `<!-- stack:managed:start -->`/`<!-- stack:managed:end -->` markers; stacked branches include parent/child links, and user-provided body text is appended below it.
- Stack-generated PR/compare links now URL-encode branch path segments and escape markdown link labels in generated bodies to avoid malformed or injected markdown links.
- Rendered HTTP(S) remote links are structurally parsed and stripped of user information, query strings, and fragments before they are shown in the terminal, opened in a browser, or embedded in generated PR content; Git operations continue to use the original remote URL.
- In styled TTY output, existing `stack pr` hashes are rendered as clickable links.
- `--debug` prints full provider/gh parse error details where non-debug mode uses concise fallback warnings.
- `stack sync` with `--debug` prints timing metrics to stderr, including totals (`plan_ms`, `apply_ms`, `total_ms`) plus plan breakdown (`setup_ms`, `pr_lookup_ms`, `assemble_ms`).
- In coloured TTY output, stack/compare links use clickable OSC 8 hyperlinks instead of raw URL text.
- In stack tree output, branches without a PR show a clickable `[no PR]` compare label.
- Stack tree output no longer shows a separate `PR:none` badge; `[no PR]` is the single missing-PR indicator.
- `SYNC:never` means a branch has not yet been synced by `stack sync` (no last-synced SHA recorded).
- Interactive prompt Ctrl-C handling uses the Dialoguer workaround from `console-rs/dialoguer#294`:
  - install a no-op `ctrlc` handler at startup,
  - on prompt errors, call `dialoguer::console::Term::stdout().show_cursor()` and `Term::stderr().show_cursor()`.
  Keep this behaviour unless prompts are migrated off Dialoguer.

## Testing focus
When adding features, prefer tests in the same module (`mod tests`).
Prioritize:
- stack graph invariants,
- sync planning and replay fallback paths,
- non-interactive CLI behaviour.
