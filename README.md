> **Live listening moved out.** The "listen to the sermon and put the verses on screen" feature (`versevision listen`: Whisper speech
> recognition, spoken-reference and verse-content matching, following a passage as it is read, version selection) is now its own project:
> [huntrw6/versevision-stt](https://github.com/huntrw6/versevision-stt). This repository is the sermon-notes importer only (since 0.16.0).

# VerseVision CLI

VerseVision is a Python 3.12+ command-line application for turning Bible references in ordinary documents into explicitly targeted ProPresenter slide imports. It is designed to work without AI, UI automation, AppleScript, simulated input, or accessibility permissions.

The current implementation includes:

- `.txt`, `.md`, `.html`, `.docx`, embedded-text `.pdf`, stdin, and direct-text extraction.
- Grammar-based parsing of canonical and composite references, version detection, validation, and stable source offsets.
- Typed configuration, translation resolution, one-verse-per-slide planning, and structured audit models.
- A typed asynchronous ProPresenter API client for version, library, presentation, path, and playlist discovery.
- Explicit UUID/name/library target resolution with ambiguity protection and fuzzy suggestions.
- Provider-backed or `--no-fetch` dry-run imports that cannot modify ProPresenter files or playlists.
- Local USFM, API.Bible, and YouVersion providers, explicit version routing, SQLite verse caching, and attribution footers.
- A compatibility-gated protobuf writer foundation with backup, temporary-file verification, atomic replacement, and UUID refresh behavior.

## Development

```powershell
uv sync --extra dev
uv run pytest
uv run versevision parse --text "John 1:14-18 & 20-23 & 25 NLT" --output json
uv run versevision import --text "John 3:16-18 NIV" --dry-run --no-fetch --non-interactive --output json
uv run versevision doctor --port 1025 --output json
uv run versevision providers test
uv run versevision cache inspect
uv run versevision template inspect --target-presentation Verses
uv run versevision template select --target-presentation Verses
uv run versevision template validate
```

After providers and a template are configured, a production import requires an explicit destination and confirmation:

```powershell
uv run versevision import sermon.pdf `
  --target-uuid PRESENTATION_UUID `
  --yes
```

If ProPresenter is running, VerseVision asks the operator to close it completely before writing and reopen it afterward. It then verifies every generated slide through the Network API. Noninteractive production writes also require ProPresenter to be closed. Failed API verification triggers a guided rollback with exit code 8; the CLI never modifies playlists as a side effect of presentation import.

Run `uv run versevision --help` for the command tree. Omit `--no-fetch` to use configured providers.

Example local USFM configuration:

```toml
[bible.version_routes]
NKJV = "local_nkjv"

[bible.providers.local_nkjv]
type = "usfm"
version = "NKJV"
path = "C:/Authorized Scripture/NKJV-USFM"
copyright = "Required copyright notice"
```

Example API.Bible configuration (the key itself remains in the environment):

```toml
[bible.version_routes]
NIV = "api_bible"

[bible.providers.api_bible]
type = "api_bible"
api_key_env = "API_BIBLE_API_KEY"

[bible.providers.api_bible.bible_ids]
NIV = "the-id-returned-by-your-api-key"
```

Example YouVersion configuration (licensed IDs may be aliased to sermon abbreviations):

```toml
[bible.version_routes]
NIV = "youversion"

[bible.providers.youversion]
type = "youversion"
api_key_env = "YOUVERSION_API_KEY"
base_url = "https://api.youversion.com/v1"

[bible.providers.youversion.bible_ids]
NIV = "111"
```

Template selection stores an exact presentation UUID, cue UUID, the named `Reference` and `Verse` objects, and a style/geometry fingerprint. Generated continuation slides repeat the reference and prefer punctuation-aware balanced breaks. The default fitting policy allows five verse-body lines plus the reference line:

```toml
[slides]
max_body_lines = 5
line_width_units = 48.0
prefer_punctuation_breaks = true
repeat_reference_on_continuation = true
```

## Quick start (sermon notes)

```bash
versevision node                          # check the connection to the ProPresenter computer
versevision template setup -t Verses --verse-slide 2   # pick the verse and sermon-point template slides
versevision run notes.pdf -t Verses --dry-run          # plan and report; changes nothing
versevision run notes.pdf -t Verses                    # import (closes and reopens ProPresenter)
versevision history                                    # earlier imports
versevision undo IMPORT_ID                             # remove exactly the slides that import added
versevision serve                                      # local web page: drag and drop, review, import
```

`run` reads `.pdf .docx .txt .md .html`, plans verse and sermon-point slides in document order, and imports them. Importing the same file again **replaces** its earlier slides instead of adding a second copy (`--no-replace` to append). `--output json` gives a machine-readable report for scripts.

Exit codes: `0` success, `10` finished but some lines were flagged, `3` flagged lines blocked the import (rerun with `--yes`), `1` general failure, `4` Bible provider, `5` ProPresenter connection, `7` write failure, `8` verification failed and the original was restored, `2` bad arguments.

### How notes are read

- A reference becomes verse slides. A verse with no version of its own uses the nearest version listed above it, then `--default-version` / `bible.default_version`. Two versions of the same passage back to back are treated as a comparison and do not change the version for later verses.
- A numbered or lettered line (`1)`, `#2:`, `a)`) becomes a point slide that keeps its label. A numbered line out of sequence (for example thesis numbers inside point 2) becomes a list-item slide in the same format. Bullets keep a bullet. A `[slide] text` line becomes a bulleted slide.
- Stage cues (`Show Photo...`, `When I say...`, `Start...`, URLs), notes to self (`NOTE:`, `TODO`, `(ask Amy...)`), email headers, titles and `Recap` headings are left out and listed in the report.
- Anything unclear is **flagged**, never guessed: bare chapters (`Psalm 23`), references mentioned inside a sentence, quoted text with no reference, unlabeled prose, and short unlabeled lines (a heading, or a point missing its number?). Only one- or two-word capitalized lines are skipped silently as headings.
- Duplicates are kept on purpose. Every verse that cannot be added (unknown version, provider failure) is listed as **not added** with the reason. A run with flagged lines stops until you pass `--yes`.
- Version labels are spell-checked, but a real version name is never altered, even without a provider (`CSB` stays `CSB`). `YLT` is read as `NLT` (`bible.version_aliases`); plain `NASB` is `NASB2020`. Slides carry only the reference and verse text, never a copyright line. ESV text is never cached.

### Where ProPresenter runs

```toml
[propresenter]
port = 1025

[node]
type = "hermes-remote"        # or "local" when ProPresenter runs on this computer
name = "jpl-graphics-imac"
app_path = "/Applications/ProPresenter 19.1.app"
```

For a remote Mac VerseVision reads the presentation file, backs it up, writes the new slides, installs the file only if it did not change meanwhile, reopens ProPresenter and checks every new slide through the ProPresenter API. If that check fails it restores the backup. If ProPresenter shows an "Are you sure you want to quit?" or "Save changes?" prompt, someone at that computer must answer it; VerseVision then stops with nothing changed.

### Web page

`versevision serve` listens on `127.0.0.1:8777`. Drop a file, review what was understood and what was flagged, then import. Use `--host 0.0.0.0` to open it to your network; it then requires the private access token printed at startup.

## Safety and compatibility

Direct `.pro` writing is an unofficial ProPresenter integration. The Windows feasibility spike passed against ProPresenter 19.0.1, API v1, and the locally observed document version; macOS acceptance, named-object template validation, provider integration, and expanded golden fixtures remain pending. The production import command therefore does not yet invoke the writer.

The writer never edits bytes in place: it locks the target, fingerprints the source, creates a backup, writes a same-directory temporary file, reparses and verifies it, checks for concurrent changes, then atomically replaces the original. Unknown document versions are rejected by default. ProPresenter must be fully closed for the write. Version 19.0.1 retained its in-memory copy and created duplicate `.pro.pro` files when an externally replaced presentation was opened, so merely switching to another presentation is not safe. See [the feasibility report](docs/FEASIBILITY.md) for the tested matrix and remaining gates.

No copyrighted Bible text is bundled. The repository's USFM acceptance corpus contains synthetic verse-like text only. Provider attribution is retained on planned slides and API keys are never serialized into configuration or error output.

## Installing on a Mac (update and uninstall)

Everything lives in the user's home folder; no admin rights are needed.

```sh
sh scripts/deploy-to-node.sh jpl-graphics-imac     # install, or update to the current code (settings and history are kept)
```

On the Mac itself, `sh install-macos.sh install versevision-*.whl` does the same from a built wheel.

| What | Where on macOS |
|---|---|
| Program | `~/.local/bin/versevision` (managed by `uv tool`) |
| Settings and API keys | `~/Library/Application Support/versevision/config.toml`, `keys.env` |
| Import history (for Undo) | `~/Library/Application Support/versevision/imports.json` |
| Backups and helper | `~/VerseVision-backups`, `~/VerseVision-tools/vvdialog` (VerseVision STT, the listener, keeps its own files in `~/VerseVision-STT`) |
| Start at login | `~/Library/LaunchAgents/org.versevision.serve.plist` (`versevision autostart enable/disable`) |

Open the page at `http://127.0.0.1:8777/` on that Mac.

```sh
sh install-macos.sh uninstall          # removes the program and the login item; keeps settings, history, backups
sh install-macos.sh uninstall --purge  # removes everything above
```


## Replace and Append never restart ProPresenter

ProPresenter ignores changes made to a file it already has open, but it lists a *new* file straight away. So every action writes a new file:

| Action | Result |
|---|---|
| New | A new presentation named after the document. |
| Replace | A copy of the chosen presentation with its verse and sermon-point slides removed and the new ones added. Blank, background, title and media slides stay. |
| Append | A copy of the chosen presentation with the new slides at the end. |

Copies are saved next to the original as `Name 2`, `Name 3`, and so on, and are added to every playlist that holds the original, directly under it (`--no-playlist` skips this). The original file is never touched. `Undo` deletes the copy and takes it out of the playlist.

`--in-place` keeps the older behaviour of editing the original file (this closes and reopens ProPresenter).

## Logs: auditing a run

Every import, dry run and undo gets its own folder, so any run can be checked afterwards.

| Where | Path |
|---|---|
| macOS | `~/Library/Logs/versevision/runs/<run id>/` |
| Linux | `~/.local/state/versevision/log/runs/<run id>/` |

| File | What it holds |
|---|---|
| `summary.txt` | Plain-language account: result, document, action, every slide, flagged / adjusted / skipped lines, each Bible lookup (provider, live or cache, time), what was written and whether ProPresenter confirmed it, a timeline |
| `events.jsonl` | Every step with a timestamp: document read, each lookup, each command sent to ProPresenter, playlist changes, errors with their traceback |
| `source.txt` | The exact text read from the document |
| `report.json` | The full plan |

```sh
versevision log                      # the latest run's summary
versevision log list                 # recent runs, newest first
versevision log show <run id>        # one run (the end of the id is enough)
versevision log show --problems      # only warnings and errors
versevision log show --events        # every step
versevision log show --path          # the folder
```

The web page shows the run id on the review screen and lists runs under History > Logs. Logs never contain API keys, tokens or file contents; the last 200 runs are kept. The older `audit.jsonl` one-line-per-import file is still written.

## Keeping the template slides safe

The verse and sermon-point template slides can live in their own presentation (default name `VerseVision Template`), so cleaning up or replacing a sermon can never delete them:

```sh
versevision template new-file --source-file /path/to/a/backup.pro   # or: --source "Verses"
```

This writes the small template presentation into the Verses library and points the saved settings at it. A template slide is never treated as a generated slide, so Replace skips it even if it is still in the same file as a sermon.
