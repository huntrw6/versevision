#!/bin/sh
# VerseVision on macOS: install, update or remove. Everything lives in your home folder; no admin rights.
#   install/update:  sh install-macos.sh install  versevision-*.whl
#   remove:          sh install-macos.sh uninstall [--purge]
# Installed files: ~/.local/bin/versevision, ~/.local/share/uv/tools/versevision, ~/.config/versevision,
#                  ~/.local/share/versevision (history), ~/VerseVision-backups, ~/VerseVision-tools.
# `uninstall` removes the program and the start-at-login item. --purge also removes settings, keys,
# history and helper tools. ProPresenter backups in ~/VerseVision-backups are only removed with --purge.
set -eu
export PATH="$HOME/.local/bin:$PATH"
cmd="${1:-}"; shift || true
case "$cmd" in
  install)
    wheel="${1:?usage: install-macos.sh install path/to/versevision.whl}"
    command -v uv >/dev/null 2>&1 || { echo "Installing uv (Python tool manager) into ~/.local/bin ..."; curl -LsSf https://astral.sh/uv/install.sh | env UV_NO_MODIFY_PATH=1 sh; }
    uv tool install --force --python 3.12 "$wheel"
    mkdir -p "$HOME/.config/versevision" && chmod 700 "$HOME/.config/versevision"
    echo "Installed: $(versevision --version 2>/dev/null || echo ok)  ($HOME/.local/bin/versevision)"
    ;;
  uninstall)
    versevision autostart disable >/dev/null 2>&1 || true
    launchctl bootout "gui/$(id -u)/org.versevision.serve" >/dev/null 2>&1 || true
    rm -f "$HOME/Library/LaunchAgents/org.versevision.serve.plist"
    uv tool uninstall versevision >/dev/null 2>&1 || true
    if [ "${1:-}" = "--purge" ]; then
      rm -rf "$HOME/.config/versevision" "$HOME/.local/share/versevision" "$HOME/.local/state/versevision" "$HOME/VerseVision-tools" "$HOME/VerseVision-backups"
      echo "Removed VerseVision and all of its settings, history, helpers and backups."
    else
      echo "Removed the VerseVision program and start-at-login item. Settings, history and backups were kept (use --purge to remove them)."
    fi
    ;;
  *) echo "usage: install-macos.sh install WHEEL | uninstall [--purge]"; exit 2 ;;
esac
