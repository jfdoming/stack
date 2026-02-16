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
- Omitting `stack track <branch>` follows create/delete selection behaviour: assume when only one viable branch exists, otherwise prompt in TTY mode.
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
