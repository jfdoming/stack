# Development Workflow

## Core commands
- `cargo build`: compile the binary.
- `cargo test`: run unit tests.
- `cargo fmt`: apply rustfmt formatting.
- `cargo run -- --help`: top-level command help.
- `cargo run -- sync --dry-run`: preview sync plan without execution.

## Install from source
- `./scripts/install.sh`: build release binary and install to `~/.local/bin/stack`.
- `./scripts/install.sh --write-shell-config`: also append PATH update to shell config.
- `STACK_INSTALL_PREFIX=/custom/prefix ./scripts/install.sh`: install under a custom prefix.

## Local behavior notes
- `stack` without args prints a one-shot tree visualization by default.
- `stack --interactive` opens the fullscreen TUI.
- Non-interactive contexts fall back to plain text (or JSON with `--porcelain`).
- `stack sync` supports staged application; use `--yes` to auto-confirm.

## Testing focus
When adding features, prefer tests in the same module (`mod tests`).
Prioritize:
- stack graph invariants,
- sync planning and replay fallback paths,
- non-interactive CLI behavior.
