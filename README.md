# stack

`stack` helps you manage stacked pull requests from the terminal.
Learn more about stacked PR workflows at [stacking.dev](https://www.stacking.dev/).

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
stack create --parent main --name feat/child
stack up               # switch to direct child in stack
stack down             # switch to direct parent in stack
stack top              # jump to top-most descendant
stack bottom           # jump to root ancestor
stack track feat/child
stack track --all --dry-run
stack untrack <branch>
stack --yes delete <branch>
stack pr --dry-run
stack pr
stack sync --dry-run
stack doctor
stack completions zsh > ~/.zsh/completions/_stack
```
