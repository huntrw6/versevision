# VerseBridge CLI

VerseBridge is a Python 3.12+ command-line application for turning Bible references in ordinary documents into explicitly targeted ProPresenter slide imports. It is designed to work without AI, UI automation, AppleScript, simulated input, or accessibility permissions.

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
uv run versebridge parse --text "John 1:14-18 & 20-23 & 25 NLT" --output json
uv run versebridge import --text "John 3:16-18 NIV" --dry-run --no-fetch --non-interactive --output json
uv run versebridge doctor --port 1025 --output json
uv run versebridge providers test
uv run versebridge cache inspect
uv run versebridge template inspect --target-presentation Verses
uv run versebridge template select --target-presentation Verses
uv run versebridge template validate
```

After providers and a template are configured, a production import requires an explicit destination and confirmation:

```powershell
uv run versebridge import sermon.pdf `
  --target-uuid PRESENTATION_UUID `
  --yes
```

If ProPresenter is running, VerseBridge asks the operator to close it completely before writing and reopen it afterward. It then verifies every generated slide through the Network API. Noninteractive production writes also require ProPresenter to be closed. Failed API verification triggers a guided rollback with exit code 8; the CLI never modifies playlists as a side effect of presentation import.

Run `uv run versebridge --help` for the command tree. Presentation writing remains disabled, so `import` currently requires `--dry-run`. Omit `--no-fetch` to use configured providers.

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

## Safety and compatibility

Direct `.pro` writing is an unofficial ProPresenter integration. The Windows feasibility spike passed against ProPresenter 19.0.1, API v1, and the locally observed document version; macOS acceptance, named-object template validation, provider integration, and expanded golden fixtures remain pending. The production import command therefore does not yet invoke the writer.

The writer never edits bytes in place: it locks the target, fingerprints the source, creates a backup, writes a same-directory temporary file, reparses and verifies it, checks for concurrent changes, then atomically replaces the original. Unknown document versions are rejected by default. ProPresenter must be fully closed for the write. Version 19.0.1 retained its in-memory copy and created duplicate `.pro.pro` files when an externally replaced presentation was opened, so merely switching to another presentation is not safe. See [the feasibility report](docs/FEASIBILITY.md) for the tested matrix and remaining gates.

No copyrighted Bible text is bundled. The repository's USFM acceptance corpus contains synthetic verse-like text only. Provider attribution is retained on planned slides and API keys are never serialized into configuration or error output.
