# Development Workflow

## Core commands
- `cargo build`: compile the binary.
- `cargo test`: run unit tests.
- `cargo fmt`: apply rustfmt formatting.
- `cargo run -- --help`: top-level command help.
- `cargo run -- sync --dry-run`: preview sync plan without execution.

## Local behavior notes
- `stack` without args launches TUI in interactive terminals.
- Non-interactive contexts fall back to plain text (or JSON with `--porcelain`).
- `stack sync` supports staged application; use `--yes` to auto-confirm.

## Testing focus
When adding features, prefer tests in the same module (`mod tests`).
Prioritize:
- stack graph invariants,
- sync planning and replay fallback paths,
- non-interactive CLI behavior.
