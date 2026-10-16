#!/usr/bin/env sh
# Builds a self-contained `versebridge` executable that includes every provider key you have.
# Keys come from the environment or from ~/.config/versebridge/keys.env (NAME=value lines).
set -eu
cd "$(dirname "$0")/.."
KEYS="${VERSEBRIDGE_KEYS:-$HOME/.config/versebridge/keys.env}"
MODULE="src/versebridge/_bundled_credentials.py"
trap 'rm -f "$MODULE"' EXIT
: > "$MODULE"
found=0
for NAME in API_BIBLE_API_KEY YOUVERSION_API_KEY ESV_API_KEY; do
  VALUE="$(printenv "$NAME" || true)"
  if [ -z "$VALUE" ] && [ -f "$KEYS" ]; then
    VALUE="$(grep -E "^[[:space:]]*$NAME=" "$KEYS" | head -1 | cut -d= -f2- | sed -e "s/^[[:space:]\"']*//" -e "s/[[:space:]\"']*$//")"
  fi
  if [ -n "$VALUE" ]; then
    python3 - "$NAME" "$VALUE" >> "$MODULE" <<'PY'
import sys
print(f"{sys.argv[1]} = {sys.argv[2]!r}")
PY
    found=$((found + 1))
  else
    echo "warning: $NAME was not found; the executable will not include it" >&2
  fi
done
[ "$found" -gt 0 ] || { echo "No provider keys found." >&2; exit 1; }
uv sync --extra dev
uv run pyinstaller --onefile --name versebridge --paths src/versebridge/presentation/generated \
  --add-data "src/versebridge/web/index.html:versebridge/web" \
  --hidden-import versebridge._bundled_credentials src/versebridge/cli.py
