#!/usr/bin/env bash
set -euo pipefail

BIN_NAME="stack"
PREFIX="${STACK_INSTALL_PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
SHELL_NAME="$(basename "${SHELL:-}")"

usage() {
  printf 'Usage: %s [--write-shell-config]\n' "$0"
}

write_shell_config=false
while (($#)); do
  case "$1" in
    --write-shell-config)
      write_shell_config=true
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      printf 'error: unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

if [[ "$BIN_DIR" == *:* ]]; then
  printf "error: install bin directory cannot contain ':' because PATH uses it as a separator\n" >&2
  exit 2
fi
if [[ "$BIN_DIR" == *$'\n'* || "$BIN_DIR" == *$'\r'* ]]; then
  printf 'error: install bin directory cannot contain newline characters\n' >&2
  exit 2
fi

printf -v quoted_bin_dir '%q' "$BIN_DIR"
export_line="export PATH=${quoted_bin_dir}:\$PATH"

fish_quote() {
  local escaped=${1//\\/\\\\}
  escaped=${escaped//\'/\\\'}
  printf "'%s'" "$escaped"
}

fish_line="fish_add_path -- $(fish_quote "$BIN_DIR")"

echo "Building release binary..."
cargo build --release --locked

mkdir -p "$BIN_DIR"
install -m 0755 "target/release/$BIN_NAME" "$BIN_DIR/$BIN_NAME"

echo "Installed $BIN_NAME to $BIN_DIR/$BIN_NAME"

path_has_bin_dir=false
IFS=: read -r -a path_entries <<< "${PATH:-}"
for path_entry in "${path_entries[@]}"; do
  if [[ "$path_entry" == "$BIN_DIR" ]]; then
    path_has_bin_dir=true
    break
  fi
done

if [[ "$path_has_bin_dir" == false ]]; then
  if [[ "$SHELL_NAME" == "fish" ]]; then
    suggested_line="$fish_line"
  else
    suggested_line="$export_line"
  fi
  echo
  echo "Add this to your shell config to use '$BIN_NAME' from anywhere:"
  printf '  %s\n' "$suggested_line"

  if [[ "$write_shell_config" == true ]]; then
    case "$SHELL_NAME" in
      zsh) rc_file="$HOME/.zshrc" ;;
      bash) rc_file="$HOME/.bashrc" ;;
      fish) rc_file="$HOME/.config/fish/config.fish" ;;
      *) rc_file="" ;;
    esac

    if [[ -n "$rc_file" ]]; then
      if [[ "$SHELL_NAME" == "fish" ]]; then
        config_line="$fish_line"
      else
        config_line="$export_line"
      fi
      mkdir -p "$(dirname "$rc_file")"
      touch "$rc_file"
      if ! grep -Fqx -- "$config_line" "$rc_file"; then
        printf '%s\n' "$config_line" >> "$rc_file"
      fi
      echo "Updated $rc_file"
    else
      echo "Could not determine shell config file automatically; add PATH manually."
    fi
  fi
else
  echo "$BIN_DIR is already in PATH"
fi
