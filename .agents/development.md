# Development Workflow

## Core commands
- `cargo build`: compile the binary.
- `cargo test`: run unit tests.
- `cargo fmt`: apply rustfmt formatting.
- `cargo run -- --help`: top-level command help.
- `cargo run -- sync --dry-run`: preview sync plan without execution.
- `cargo run -- track feat/branch --parent main`: track an existing local branch under a parent branch.
- `cargo run -- track --all --dry-run`: preview inferred relationships for all local non-base branches.
- `cargo run -- untrack feat/branch`: remove a tracked branch record and splice children to its former parent.
- `cargo run -- completions zsh`: print shell completion script (works for `bash`, `zsh`, `fish`, `elvish`, `powershell`).
- `cargo run -- --yes delete <branch>`: close/delete PR, splice branch from stack, and remove local branch.

## Install from source
- `./scripts/install.sh`: build release binary and install to `~/.local/bin/stack`.
- `./scripts/install.sh --write-shell-config`: also append PATH update to shell config.
- `STACK_INSTALL_PREFIX=/custom/prefix ./scripts/install.sh`: install under a custom prefix.

## Local behaviour notes
- `stack` without args prints a one-shot tree visualization by default.
- `stack --interactive` opens the fullscreen TUI.
- Non-interactive contexts fall back to plain text (or JSON with `--porcelain`).
- `stack sync` supports staged application; use `--yes` to auto-confirm.
- `stack track` records relationships for existing local branches; it can infer parents from PR base metadata and git ancestry.
- In single-branch track mode, parent inference is attempted by default when `--parent` is omitted.
- Omitting `stack track <branch>` follows create/delete selection behaviour: assume when only one viable branch exists, otherwise prompt in TTY mode.
- In non-interactive mode, if track auto-assumes a single viable target branch and would mutate state, pass `--yes` or an explicit target branch.
- If default inference cannot resolve a parent, single-branch track falls back to the same assumption/prompt flow for parent selection.
- Omitting `stack untrack <branch>` follows the same assumption/prompt flow as delete.
- In non-interactive mode, if untrack auto-assumes a single viable target branch, pass `--yes` or an explicit target branch.
- Omitting `stack completions <shell>` prompts for shell selection in TTY mode.
- `stack pr` requires the current branch to be tracked with a tracked parent, and skips PR creation when an existing PR already matches the branch head.
- `stack pr` requires confirmation before creating a PR unless `--yes` is passed.
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
