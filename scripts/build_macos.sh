#!/usr/bin/env sh
set -eu
uv sync --extra dev
uv run pyinstaller --onefile --name versebridge src/versebridge/cli.py

