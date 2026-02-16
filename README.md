# stack

`stack` is a Rust CLI for managing stacked pull request workflows with a repo-local SQLite database (`.git/stack.db`).

## Install from source

### Quick install (recommended)
```bash
./scripts/install.sh
```

This builds a release binary and installs it to `~/.local/bin/stack` by default.

If `~/.local/bin` is not in `PATH`, the script prints the exact line to add.
You can also ask it to update shell config automatically:

```bash
./scripts/install.sh --write-shell-config
```

Optional install prefix:

```bash
STACK_INSTALL_PREFIX="$HOME/.cargo" ./scripts/install.sh
```

### Manual install
```bash
cargo install --path .
```

## Common commands
```bash
stack                 # one-shot stack visualization
stack --interactive   # fullscreen interactive UI
stack create
stack --yes delete <branch>
stack pr --dry-run
stack sync --dry-run
stack doctor
stack completions zsh > ~/.zsh/completions/_stack
```
