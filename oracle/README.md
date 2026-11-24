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

Whole documents, `cases/<name>/`: 213 cases (each has `input.<ext>`, `options.toml`, `bible.json` and `expected/`: `extracted.txt` (the text the reader returned, which the classifier receives), `document.json` (the classifier's items, issues and the rule of every paragraph), `plan.json`, `review.json`, `trace.jsonl`)

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
| `classifier-*` | 84 | small documents, one per rule branch of `notes/classifier.py` that the cases above never reach (found with `python/scripts/classifier_coverage.py`) and per setting: `[slide]` tags, references in the middle of a sentence, quotes (above a reference, alone), lead and trailing reference chains, references in brackets, bad references, empty and one-line documents, the lines before the first item, headings and ALL CAPS lines, `Recap` and dates, stage cues, extra cue words, skip patterns, markers and `KEY #n)` headers, labelled lines (`Conclusion:`), one slide per line break or one paragraph per point, duplicates, wrapped (PDF/email) lines, odd whitespace and case-insensitive Unicode. They run with `no_fetch` (`bible.json` is `{}`): what they test is the classifier |

Line level, one JSON object per line:

| file | lines | what |
|---|---|---|
| `references.jsonl` | 132 | `parse_references` of one line: every line of the reference-parser tests that contains a digit, extra lines (abbreviations, ranges across chapters, `&`/`;`/`,` lists, invalid chapters and verses, unknown books), and every line with a digit in the Whitefield and kitchen-sink documents; 7 have issues, 2 make the Python parser raise (`John 0:1`, `John 1:0`), 6 use version aliases |
| `fitting.jsonl` | 405 | `balance_lines` 108, `split_verse_text` 108, `split_point_text` 108 (each over 27 texts and 4-5 parameter sets, including `tolerance`/`punctuation_tolerance` 0.0, 0.05, 0.1, 0.15, 0.25, 0.5, 0.6), plus `text_width_units` 27 and `wrap_body_lines` 54 |
| `document_errors.jsonl` | 71 | documents and options for which `classify_document` raises: the reference parser (chapter 0), a `skip_patterns` entry that `re.compile` rejects (the exact message), and two crashes of the Python importer (a numbered whole-chapter reference, a numbered duplicate that is dropped) |
| `user_patterns.jsonl` (+ `user_patterns_subjects.json`) | 520 | Python `re` as the user writes it in `notes.skip_patterns`: 261 patterns, with and without IGNORECASE (4 of the rows are compile errors), each searched in 55 texts: the span and groups of `search`, or the exact compile error |
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

Since S3 every case runs the whole planner (`workflow::plan_notes`, `build_report`, and `apply_review_choices` where the case has `choices.json`) and compares
`plan.json`, `review.json`, the whole `trace.jsonl` and `plan_after_choices.json`; the list is empty (213/213). The document readers are S4: the runner hands the
planner `expected/extracted.txt` (what the reader returned) and takes the reader's own `extract` record (seq 1) from `expected/trace.jsonl`, so a case does
not depend on a reader. `group:`/`case:` entries are that full comparison. The notes classifier (S2b) is also checked on its own, from `expected/extracted.txt`:
`document:<case-or-group>` is `expected/document.json` of that case, and `trace-stage:<name>` is one of the classifier's trace stages (`parse_start`,
`line_join`, `paragraph`, `item`) over every case. `ORACLE_VERBOSE=1` prints every failing line of the line-level files, not only the first.

## The Bible providers: `oracle/providers/` (S5)

What the Python providers do with recorded-shape responses (every text is made up, every key is a fake, there is no real call: the HTTP side is
`httpx.MockTransport`). `python/scripts/oracle_providers.py` writes it from `export_oracle.py`; `crates/versevision-bible/tests/oracle_providers.rs` and `keys.rs`
compare the Rust providers (against a local HTTP server that plays the same responses) and print the counts.

| file | rows | what |
|---|---|---|
| `html_text.jsonl` | 255 | API.Bible verse `content` -> text: `BeautifulSoup(..., "html.parser").get_text(" ", strip=True)` and the leading `[16]` marker removed (tags, entities, numeric references, comments, declarations, CDATA, `script`/`style`/`template`/`rt`/`rp`, broken markup) |
| `json_errors.jsonl` | 112 | `httpx.Response.json()` on a body: valid, or the exact `JSONDecodeError` / `UnicodeDecodeError` text the planner shows after `response failed:` |
| `keys.jsonl` | 36 | `credentials._from_keys_file` on the text of a `keys.env` for three names (quotes, comments, blank and odd lines, line ends) |
| `usfm.jsonl` | 8 scenarios | `LocalUsfmProvider` over `oracle/providers/usfm/*` (written by the exporter) and `python/tests/fixtures/usfm/synthetic`: versions, health, passages, errors. `{path}` and `{resolved}` stand for the folder as configured and as resolved |
| `api_bible.jsonl`, `youversion.jsonl`, `esv.jsonl` | 35 / 18 / 11 scenarios | one provider against canned responses: list of Bibles, passages, configured ids, listing shapes, every status (401, 403, 429, 5xx, 3xx not followed, 204), timeouts, no retry, bad JSON, no key. Each row has the requests the provider made (method, path, sorted query, key header) as well as the results |
| `registry.jsonl` | 4 scenarios | `ProviderRegistry._fetch` over several providers and a real SQLite cache: provider, source (`live`, `cache`, `live (not stored)`), verses, the routing errors, what was cached (the ESV text never) |

A Python quirk the files record: a provider response of an unexpected shape (a list where an object is expected) raises `AttributeError` in Python, not
`BibleProviderError`; no row has such a response, the Rust providers answer `... response failed: unexpected response shape; ...` (own tests).

## The ProPresenter files: `oracle/pro/` (S6a)

What the Python file writer (`presentation/protobuf_writer.py`, with `themes.py`) writes for a slide plan, byte for byte. `python/scripts/oracle_pro.py`
writes it from `export_oracle.py`; `crates/versevision-pp/tests/oracle_pro.rs` runs the Rust writer on the same inputs and prints
`oracle pro: N/N cases byte-identical`. Randomness is injectable: `protobuf_writer.new_uuid()` (defaults to `uuid.uuid4`) is replaced by the exporter with
a seeded source, the n-th id of a case being the version-4 uuid made from the first 16 bytes of `sha256("<case>:<n>")` (`SeededUuids` in Rust), and the
module's clock is frozen (`now` in `options.toml`).

| path | what |
|---|---|
| `files/*.pro` | the presentations the cases start from (built by the exporter from `tests/fixtures/sample/Sample.pro` and `propresenter19/Verses.original.pro`: a service with blank, title, shape, non-slide, plain and themed generated slides and an arrangement; the same without templates; an empty one; one with a `-0.0` float and unknown fields; a Windows-style one; files from another ProPresenter version; broken templates). A case may also name a fixture (`fixture:sample/Sample.pro`) |
| `themes/` | a copy of ProPresenter's Themes folder: the three real theme files and made-up ones (a two-box layout, several slides, a slide without a text box, groups, a name used in two groups, an unreadable file, a `Theme` at the top) |
| `themes.json` | what `themes.py` makes of that folder: every theme slide (group, theme, name, number, boxes, font, size, `has_reference_box`), the rows of the look picker (`/api/themes`) and 31 `find_slide` answers and error texts |
| `candidates.json` | `inspect_template_candidates` / `inspect_point_candidates` (with the style fingerprints) of every file above |
| `cases/<case>/` | `options.toml` (`mode`: append, replace, derive, create, template_file, synthetic; the files; the saved template definition; the looks), `plan.json` (a slide plan), `expected.pro` (the bytes) and `expected.json` (the `PlanWriteResult` without paths, or the error as `"<ExceptionName>: <message>"`, and what `validate_template` says) |

The 60 cases: Append and Replace (the template in the target or in another file, other slides kept, a group shared with a slide that stays), derive (fresh id
and `swap_keeps_id`, Replace and Append), create, the template-only file, the feasibility slide, real plans from `oracle/cases` (the whole Whitefield notes,
a long verse cut into several slides, points limited to two lines, placeholders), special characters (the apostrophes and dashes become `\uN?`, not `\'92`: that
is what Python writes), empty bodies, 200 slides, every theme layout, and the 15 cases that raise (compatibility, missing template, missing point template,
no group, two slide actions, not RTF, empty plan).

## The bible text is synthetic

`bible.json` of a case holds the verses it needs. The text is generated from the verse key (a fixed algorithm in `export_oracle.py`), so the
corpus has no Scripture text in it and no copyright to carry; it only has to have realistic length and punctuation. The few cases whose texts
come from the tests (`quote-*`) use short KJV/NKJV quotations already in the test files.

## What the oracle does NOT cover (test it another way)

* **Live Bible providers** (API.Bible, YouVersion, ESV, local USFM files; their caching, keys, errors, copyright text). The case files use a fake provider
  that serves `bible.json`. The real providers have their own oracle, `oracle/providers/` (below), recorded against canned HTTP responses; what no oracle can
  show is the real services (their live answers, rate limits, key checks).
* **The swap/transport/history pipeline** (S6b): quit/launch/verify of ProPresenter, the playlist, the swap with its rollback, history and undo, the
  run log folders. `python/tests/integration`, `test_write_workflow.py` and `test_pipeline.py` are the specification. (The files the pipeline writes
  are covered, see `oracle/pro/` below.)
* **A real ProPresenter opening the written files** (S9, on the rig). The oracle proves the Rust files are the same bytes as the Python app's; it cannot
  prove ProPresenter accepts them.
* **The web page and its server**: routes, jobs, PWA, review panel behaviour, authentication. The page itself is served unchanged from Rust (S7).
* **History and undo**, the run log folders (`summary.txt`, `events.jsonl`, `options.json`, `source.txt`, `report.json`), `explain` and `replay`
  (only `trace.jsonl` is covered, and the `review_*` trace records written by `apply_review_choices` are not).
* **The command line**, the Config page and keys.env. (The config model itself, TOML load/save, profiles and the `--set` parser are covered by the `config_*` files.)
* **Documents the extractors read differently**: only the formats in `cases/format-*` are covered (a small docx and two small pdfs). Real-world pdf
  and docx layouts (tables, text boxes, columns, scanned pages) are not.
* **Live versions of Python's libraries**: the oracle is produced with the locked environment; `difflib`, `pypdf` and `BeautifulSoup` behaviour in
  other versions is not tracked.

## The classifier's coverage

`python/scripts/classifier_coverage.py` classifies every case document with its own options and reports the coverage of `notes/classifier.py` alone
(`--without-group classifier` leaves out the `classifier-*` cases). It was 93% (26 statements and 19 branches missed) before the `classifier-*` cases
and is 98% after; what is left is not reachable from a case file: the `PointItem.display` and `NoteDocument` list properties (the planner reads them,
the oracle does not), the two older keyword shortcuts of `classify_document` (`line_mode`, `unlabeled_bullets`: not config options) and two lines that
no input reaches (a quote opener after a closed quote, which an earlier rule has already answered; an item type that is never none of the four).

## Adding a case

Edit `python/scripts/export_oracle.py` (a `text_case(...)` in `curated_cases()`, or a new input in one of the other builders), regenerate, and commit
the new files with the change. A case needs the document, the option overrides that differ from the defaults, and (when the provider must answer
something special) `overrides` or `missing_versions`.
