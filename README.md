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
stack create --insert feat/child --name feat/mid
stack up               # switch to direct child in stack
stack down             # switch to direct parent in stack
stack top              # jump to top-most descendant
stack bottom           # jump to root ancestor
stack track feat/child
stack track --all --dry-run
stack untrack <branch>
stack rename <old> <new>
stack move [target] --parent <parent>
stack split --at <commit> --name feat/part-1 --top-name feat/top
stack --yes delete <branch>
stack pr --dry-run
stack pr
stack push
stack config push-target
stack config push-target auto
stack push --push-target upstream
stack sync --dry-run
stack doctor
stack completions zsh > ~/.zsh/completions/_stack
```

## Fork and maintainer workflows

`stack` stores repository metadata in Git's shared common directory (normally `.git/stack.db`, shared by linked worktrees) and treats the push target separately from the remote tracked by the base branch:

- `auto` uses the canonical repository when GitHub reports write access; otherwise it uses the fork.
- `upstream` always selects the canonical PR repository.
- `fork` always selects the contributor fork.

The first real push prompts in an interactive terminal. Non-interactive pushes detect and cache the automatic choice. Detection is cached for 24 hours, while descendants inherit the repository already used by their nearest published stack ancestor.

Use a one-command override without changing the stored default:

```bash
stack pr --push-target upstream
stack push --push-target fork
```

Overrides fail if an existing branch upstream points to the other repository. `stack` never silently moves a published stack or falls back to a fork after a rejected upstream push.
