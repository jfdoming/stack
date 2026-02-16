#!/usr/bin/env bash
set -euo pipefail

BIN_NAME="stack"
PREFIX="${STACK_INSTALL_PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
SHELL_NAME="$(basename "${SHELL:-}")"

write_shell_config=false
if [[ "${1:-}" == "--write-shell-config" ]]; then
  write_shell_config=true
fi

echo "Building release binary..."
cargo build --release

mkdir -p "$BIN_DIR"
install -m 0755 "target/release/$BIN_NAME" "$BIN_DIR/$BIN_NAME"

echo "Installed $BIN_NAME to $BIN_DIR/$BIN_NAME"

if [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
  export_line="export PATH=\"$BIN_DIR:\$PATH\""
  echo
  echo "Add this to your shell config to use '$BIN_NAME' from anywhere:"
  echo "  $export_line"

  if [[ "$write_shell_config" == true ]]; then
    case "$SHELL_NAME" in
      zsh) rc_file="$HOME/.zshrc" ;;
      bash) rc_file="$HOME/.bashrc" ;;
      fish) rc_file="$HOME/.config/fish/config.fish" ;;
      *) rc_file="" ;;
    esac

    if [[ -n "$rc_file" ]]; then
      if [[ "$SHELL_NAME" == "fish" ]]; then
        fish_line="fish_add_path $BIN_DIR"
        grep -Fqx "$fish_line" "$rc_file" 2>/dev/null || echo "$fish_line" >> "$rc_file"
      else
        grep -Fqx "$export_line" "$rc_file" 2>/dev/null || echo "$export_line" >> "$rc_file"
      fi
      echo "Updated $rc_file"
    else
      echo "Could not determine shell config file automatically; add PATH manually."
    fi
  fi
else
  echo "$BIN_DIR is already in PATH"
fi
