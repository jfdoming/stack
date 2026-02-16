# Development Workflow

## Core commands
- `cargo build`: compile the binary.
- `cargo test`: run unit tests.
- `cargo fmt`: apply rustfmt formatting.
- `cargo run -- --help`: top-level command help.
- `cargo run -- top`: switch to the top-most descendant in the current stack path.
- `cargo run -- bottom`: switch to the root ancestor branch for the current stack path.
- `cargo run -- up`: switch to a direct child branch.
- `cargo run -- down`: switch to the direct parent branch.
- `cargo run -- sync --dry-run`: preview sync plan without execution.
- `cargo run -- track feat/branch --parent main`: track an existing local branch under a parent branch.
- `cargo run -- track --all --dry-run`: preview inferred relationships for all local non-base branches.
- `cargo run -- untrack feat/branch`: remove a tracked branch record and splice children to its former parent.
- `cargo run -- completions zsh`: print shell completion script (works for `bash`, `zsh`, `fish`, `elvish`, `powershell`).
- `cargo run -- --yes delete <branch>`: close/delete PR, splice branch from stack, and remove local branch.
- `cargo run -- --debug pr --yes`: include detailed gh parse/debug error output for PR checks.

## Install from source
- `./scripts/install.sh`: build release binary and install to `~/.local/bin/stack`.
- `./scripts/install.sh --write-shell-config`: also append PATH update to shell config.
- `STACK_INSTALL_PREFIX=/custom/prefix ./scripts/install.sh`: install under a custom prefix.

## Local behaviour notes
- `stack` without args prints a one-shot tree visualization by default.
- `stack --interactive` opens the fullscreen TUI.
- `stack up`/`stack top` prompt for child selection in TTY mode when the current branch has multiple tracked children; non-interactive mode returns an ambiguity error.
- `stack create` switches to the newly created branch and does not print an immediate compare URL because the new branch initially has no diff.
- Non-interactive contexts fall back to plain text (or JSON with `--porcelain`).
- `stack sync` supports staged application; use `--yes` to auto-confirm.
- After `stack sync` applies operations, it restores the branch that was checked out before the sync run started.
- During `stack sync`, open PR bodies are refreshed to keep the managed stack-flow section current; user-written text outside managed markers is preserved.
- Sync batches GitHub PR metadata lookups to reduce per-branch `gh` round trips on larger stacks.
- `stack track` records relationships for existing local branches; it can infer parents from PR base metadata and git ancestry.
- In single-branch track mode, parent inference is attempted by default when `--parent` is omitted.
- Omitting `stack track <branch>` follows create/delete selection behaviour: assume when only one viable branch exists, otherwise prompt in TTY mode.
- In non-interactive mode, if track auto-assumes a single viable target branch and would mutate state, pass `--yes` or an explicit target branch.
- If default inference cannot resolve a parent, single-branch track falls back to the same assumption/prompt flow for parent selection.
- Omitting `stack untrack <branch>` follows the same assumption/prompt flow as delete.
- In non-interactive mode, if untrack auto-assumes a single viable target branch, pass `--yes` or an explicit target branch.
- `stack untrack main` is allowed as a no-op and reports that the base branch remains the stack root.
- Omitting `stack completions <shell>` prompts for shell selection in TTY mode.
- On stacked branches, `stack pr` uses the tracked parent as PR base and skips opening when an existing PR already matches the branch head.
- `stack pr` pushes and auto-opens the PR URL immediately (no confirmation prompt).
- If browser auto-open fails, `stack pr` prints a manual fallback link; styled TTY output uses OSC 8 clickable text instead of truncating the URL.
- On non-stacked branches, `stack pr` warns and uses the repo base branch as PR base.
- `stack pr` blocks self-targeted PRs (`base == head`) with a clear message instead of generating a broken compare link.
- When `stack pr` exits early for self-targeted PRs (`base == head`), it suppresses the redundant non-stacked warning.
- For fork branches, `stack pr` builds compare links against `upstream` (when configured) and uses `owner:branch` head refs.
- Generated `stack pr` URLs always include a compact `Stack Flow` PR body section wrapped in `<!-- stack:managed:start -->`/`<!-- stack:managed:end -->` markers; stacked branches include parent/child links, and user-provided body text is appended below it.
- In styled TTY output, existing `stack pr` hashes are rendered as clickable links.
- `--debug` prints full provider/gh parse error details where non-debug mode uses concise fallback warnings.
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
