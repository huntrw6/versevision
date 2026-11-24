# The oracle

What the Python app (`python/`) produces for a corpus of documents and for single lines. The Rust port must reproduce all of it; the exact shapes
and the comparison rules are in [`docs/PLAN_CONTRACT.md`](../docs/PLAN_CONTRACT.md). **Nothing in this folder is edited by hand.**

## Regenerate

```sh
cd python
uv run --extra dev python scripts/export_oracle.py            # rewrites ../oracle (takes about 40 s)
git status --short ../oracle                                    # an unchanged Python app must show nothing
uv run --extra dev python scripts/export_oracle.py --out /tmp/oracle2 && diff -r ../oracle /tmp/oracle2   # the same twice
```

The exporter uses only the standard library and the app itself (plus `tomli_w` and `python-docx`, which the app already depends on). It removes
`oracle/cases` and rewrites it, so a case that was dropped from the exporter disappears. It reads the fixtures and the test files in
`python/tests/` (documents are lifted from them mechanically), so a changed test document shows up as a changed or new case. Use the locked
environment (`uv.lock`): the oracle depends on Python's `difflib` and Unicode tables, and the generated docx/pdf inputs are written byte-for-byte
reproducibly.

## What is in it

Whole documents, `cases/<name>/`: 129 cases

| group | cases | what |
|---|---|---|
| `whitefield-*` | 8 | `tests/fixtures/whitefield_notes.txt` under six option sets (defaults; `points.max_lines_per_slide=2`; `slides.max_body_lines=4`; `notes.line_breaks=paragraph`; `points.strip_trailing_period=false`; `points.fix_label_typos=false`), plus `no_fetch` and default version NIV |
| `fixture-*` | 2 | the other note files in `tests/fixtures/notes/` |
| `format-*` | 6 | the same notes as txt, md, html, docx and pdf, plus a pdf whose line was cut by the page |
| `synthetic-*` | 3 | the synthetic acceptance document: text read from the synthetic USFM fixture through the app's own USFM provider, text from the offline generator, and a narrow-slide variant |
| `tests-*` | 61 | every notes document the tests contain, lifted mechanically (multi-line string constants, joined literal lists and the first argument of the tests' `run`/`plan` helpers; named `tests-<test file>-<sha1 of the text>`) |
| `options-*` | 36 | one "kitchen sink" notes document under one changed option each (points, notes, verses, slides and bible tables) |
| `kitchen-sink-*` | 3 | the same document with defaults, with `no_fetch`, and with no default version |
| `versions-*`, `quote-*`, `version-*` | 5 | the version of a verse: nearest above, written, comparison pair, version taken from the quoted wording (KJV vs NKJV), quote check off |
| `provider-*` | 2 | a version the offline provider does not have; a version label that is not a version |
| `label-fix-*`, `long-verse-split`, `review-choices-*` | 3 | point header typos corrected to match the others, a verse cut into several slides, review choices applied after planning (`choices.json`) |

Line level, one JSON object per line:

| file | lines | what |
|---|---|---|
| `references.jsonl` | 132 | `parse_references` of one line: every line of the reference-parser tests that contains a digit, extra lines (abbreviations, ranges across chapters, `&`/`;`/`,` lists, invalid chapters and verses, unknown books), and every line with a digit in the Whitefield and kitchen-sink documents; 7 have issues, 2 make the Python parser raise (`John 0:1`, `John 1:0`), 6 use version aliases |
| `fitting.jsonl` | 405 | `balance_lines` 108, `split_verse_text` 108, `split_point_text` 108 (each over 27 texts and 4-5 parameter sets, including `tolerance`/`punctuation_tolerance` 0.0, 0.05, 0.1, 0.15, 0.25, 0.5, 0.6), plus `text_width_units` 27 and `wrap_body_lines` 54 |
| `versions.jsonl` | 82 | `canonical_version`/`correct_version`/`is_real_version` of 58 labels (every known version, case variants, edition aliases, typos, unknown), `correct_version` against an available set 6, `quote_score` 18 |

The config model (`python/src/versevision/config.py`):

| file | what |
|---|---|
| `config_defaults.json` | `AppConfig().model_dump(mode="json")`: every section, field and default |
| `config_defaults.toml` | the same, as `save_config` writes it (`tomli_w`, `exclude_none`); Rust must write the same bytes |
| `config_rich.toml` | a config that uses every kind of value (lists, providers, a template, profiles with odd names, text that needs escaping), saved by Python; Rust must read it and write the same bytes back |
| `config_options.jsonl` | `with_options(config, profile=..., sets=[...])`: `{"base", "profile", "sets", "out"}`, where `out` is `{"changed": {"a.b": value}, "removed": [...]}` (the fields that differ from the base, flattened) or `{"error": "<exact message>"}`. Includes unknown names, out-of-range numbers, lists, bool words, profile merges and the first-error order |

Python quirk the file does not record: `with_options` hands the profile's own tables to the data it edits, so a `--set` on a table that only the profile
provides changes the caller's profile too (calling it twice on one config shows it). The Rust `with_options` works on a copy; the exporter takes the
"before" side of `changed` first and gives every row its own copy of the base.

## How the Rust side is checked

`cargo test -p versevision-core --test oracle -- --nocapture` runs everything above and prints the pass count per group. `expected_failures.txt` is a ratchet:
it lists the case groups (`group:tests`), single cases (`case:...`) and files (`file:references.jsonl`) that do not pass yet. The test fails when a listed
one passes (delete the line) or an unlisted one fails, so the list only shrinks.

## The bible text is synthetic

`bible.json` of a case holds the verses it needs. The text is generated from the verse key (a fixed algorithm in `export_oracle.py`), so the
corpus has no Scripture text in it and no copyright to carry; it only has to have realistic length and punctuation. The few cases whose texts
come from the tests (`quote-*`) use short KJV/NKJV quotations already in the test files.

## What the oracle does NOT cover (test it another way)

* **Live Bible providers** (API.Bible, YouVersion, ESV, local USFM files; their caching, keys, errors, copyright text). The oracle uses a fake provider that
  serves `bible.json`. S5 tests the providers against recorded HTTP responses or its own fakes.
* **ProPresenter file writing**: the protobuf presentation writer, template cue/object detection, backups, atomic replacement, the
  swap/append/replace pipeline, the playlist, quit/launch/verify. `python/tests/integration` and `test_write_workflow.py`/`test_pipeline.py` are the
  specification for S6; the golden `.pro` files are in `python/tests/fixtures/propresenter19/`.
* **Themes** (reading and choosing slide looks): `looks` is `null` everywhere.
* **The web page and its server**: routes, jobs, PWA, review panel behaviour, authentication. The page itself is served unchanged from Rust (S7).
* **History and undo**, the run log folders (`summary.txt`, `events.jsonl`, `options.json`, `source.txt`, `report.json`), `explain` and `replay`
  (only `trace.jsonl` is covered, and the `review_*` trace records written by `apply_review_choices` are not).
* **The command line**, the Config page and keys.env. (The config model itself, TOML load/save, profiles and the `--set` parser are covered by the `config_*` files.)
* **Documents the extractors read differently**: only the formats in `cases/format-*` are covered (a small docx and two small pdfs). Real-world pdf
  and docx layouts (tables, text boxes, columns, scanned pages) are not.
* **Live versions of Python's libraries**: the oracle is produced with the locked environment; `difflib`, `pypdf` and `BeautifulSoup` behaviour in
  other versions is not tracked.

## Adding a case

Edit `python/scripts/export_oracle.py` (a `text_case(...)` in `curated_cases()`, or a new input in one of the other builders), regenerate, and commit
the new files with the change. A case needs the document, the option overrides that differ from the defaults, and (when the provider must answer
something special) `overrides` or `missing_versions`.
