# The plan contract

VerseVision is being rewritten in Rust (see `README.md`). Until the Rust app matches it, **the Python app in `python/` is the specification**.
This file says exactly what the Python app produces for a document, so the Rust port can be checked against it file by file.
The expected results live in `oracle/` (how to regenerate and what is not covered: `oracle/README.md`).

Everything below was read off the Python code and the files in `oracle/`; when this text and a file in `oracle/` disagree, the file wins
(and this text has a bug to fix).

## 1. One case

```
oracle/cases/<name>/
  input.<ext>              the document: txt, md, html, docx or pdf
  options.toml             only what differs from the defaults (may be empty); [run] holds the run parameters
  bible.json               the verse text the case needs: "<VERSION> <Book> <ch>:<v>" -> text   (a run is offline and deterministic)
  choices.json             (rare) review choices to apply after planning: {"skip": [ids], "include": [ids], "reject_fixes": [ids]}
  expected/plan.json       the plan (section 4)
  expected/review.json     the review report the web page is built from (section 5)
  expected/trace.jsonl     the decision trace (section 6)
  expected/plan_after_choices.json   (only with choices.json) the slides after the choices (section 7)
```

### How a case is run

1. **Read the document** exactly like `versevision.documents.extract_text`:
   * `.txt`, `.md`: UTF-8, read in text mode, so `\r\n` and `\r` become `\n`.
   * `.html`: BeautifulSoup (`html.parser`) `get_text("\n")`.
   * `.docx`: the text of every Word paragraph, joined with `\n` (tables, headers and text boxes are not read).
   * `.pdf`: the text layer of every page (pypdf `extract_text`), joined with `\n`; no text at all is an error.
   * then the ligature characters U+FB00..U+FB06 are replaced by `ff fi fl ffi ffl st st`.
   * The `extract` trace record is written here (section 6).
2. **Options.** Start from the default config (`python/src/versevision/config.py`: every field name and default there is part of the contract) and
   apply the tables of `options.toml` (`[points]`, `[slides]`, `[notes]`, `[verses]`, `[bible]`, ...). Missing key = the default.
   `[run]` is not config; it holds
   * `default_version` (string; default `"NKJV"`; `""` = no default version), passed as `default_version` to the planner;
   * `no_fetch` (bool; default `false`): plan without any Bible text (the verses become empty placeholders).
3. **Plan** with the app's `plan_notes(text, config, default_version=..., no_fetch=..., provider_registry=<the offline provider>)`.
   With `no_fetch = true` there is no provider and `bible.json` is `{}`.
4. **The offline provider** (this replaces the live Bible providers; the Rust runner needs the same fake):
   * A verse is looked up as `"<reference.version> <reference.book_display_name> <chapter>:<verse>"`, for example `"NKJV Psalms 23:1"`.
     The book name is the display name the parser produced (`Psalms`, `1 Corinthians`, `Song of Solomon`).
   * A passage is the list of verses of all its spans in order. A span inside one chapter is `start..=end`. A span across chapters is
     the rest of the first chapter, every verse of the chapters between, and `1..=end` of the last chapter, using the verse counts in
     `references/versification.py`.
   * Each verse comes back with `book_id`, `chapter`, `verse`, `text` from the file, `version` = the reference's version, no copyright,
     no attribution. The passage's `resolved_version` is the reference's version.
   * A key that is not in `bible.json` fails the whole passage with the message `no verse text for <first missing key>`
     (this becomes the `not added: ...` reason of the decision).
   * Every fetch writes one `provider` trace record (section 6): `provider="oracle"`, `source="bible.json"`, `ms=0`, `reference`,
     `version`, and on failure `error` instead of provider/source.
   * `bible.json` texts are **synthetic** (made up, not Scripture), except the few cases whose texts come from the tests
     (`quote-version-*`, which need real wording differences between KJV and NKJV to exercise the quote check).
   * The quote check may ask the provider for the same verse in other versions (`verses.quote_versions`); those verses are in `bible.json` too.

## 2. Encoding of the files

UTF-8, LF line ends, no BOM. JSON keys are sorted (alphabetically, by code point) and every JSON file ends with a newline. `plan.json` and
`review.json` are indented by 2 spaces; `trace.jsonl` and the `.jsonl` oracles hold one compact JSON object per line.
Floats are written the way Python writes them (`48.0`, `0.15`); compare them as numbers. Non-ASCII characters are written as themselves.
`oracle/**` is marked `-text` in `.gitattributes`: git must not change line ends in these files (the inputs may contain `\r`, and the docx and
pdf are binary).

## 3. Names

Field names are the Python model field names. Where the task text, the trace and the review use another word for the same thing:

| plan.json (`PlannedSlide`) | trace `plan` record | review.json slide |
|---|---|---|
| `kind` | `kind` | `kind` |
| `reference_text` | `reference` | `title` (reference + ` (version)`) |
| `version_text` | `version` | |
| `body_text` | `text` | `text` |
| `verse_keys` | `verses` | |

## 4. `expected/plan.json`

The `NotesImportResult` the planner returned, dumped as JSON (`model_dump(mode="json")`), without `target` and `files_changed` (always empty),
plus `looks: null` (themes are not part of this contract).

```jsonc
{
  "fetched": true,                  // false when no_fetch
  "verse_count": 17, "point_count": 3,
  "slide_plan": {
    "slides": [ { "kind": "verse" | "point",
                  "body_text": "line one\nline two",     // "\n" between lines of one slide; the line breaks are part of the contract
                  "reference_text": "1 Corinthians 10:13", // "" for a point and for a continuation slide when slides.repeat_reference_on_continuation=false
                  "version_text": "NKJV",                  // the version the verse was fetched in; "" for a point
                  "copyright_text": null,                  // always null with the offline provider
                  "source_passage_index": 4,               // 0-based: one number per verse or point entry of the document, in order
                  "verse_keys": ["1CO.10.13"] } ],         // [] for a point; BOOKID.chapter.verse
    "attribution_slides": []        // always []
  },
  "decisions": [ { "id": 0,                         // 0.. in document order
                   "line": 12,                      // 1-based line of the document after reading; 0 for an invalid reference found outside any line
                   "kind": "verse" | "point" | "left_out" | "error",
                   "title": "John 3:16-18 (NIV)",
                   "text": "John 3:16-18 NIV",       // the line as written (a point: the formatted slide text)
                   "tier": "good" | "warning" | "error",
                   "reasons": ["..."],               // exact prose, in order, without duplicates
                   "first_slide": 7, "slide_count": 3,   // 0-based index in slide_plan.slides; slide_count 0 = no slide
                   "correction": {},                  // or {"from": "KEY#2)", "to": "KEY #2)", "original_slides": ["..."]} for an automatic fix to approve
                   "add_text": "" } ],               // for a left-out line: the slide text "add this as a slide" would make ("\f" separates slides)
  "notices": [ {"line": 4, "text": "Romans 8:28", "problem": "no version listed; used NKJV (nearest version above)"} ],   // adjustments the tool made
  "skipped": [ {"line": 1, "text": "...", "problem": "header: title / date line"} ],         // left out on purpose
  "review":  [ {"line": 13, "text": "Psalm 23", "problem": "whole-chapter reference; add verse numbers to include it"} ],   // need a decision
  "failed":  [ {"line": 5, "text": "Romans 8:28 ESV", "problem": "not added: no verse text for ESV Romans 8:28"} ],            // could not be added
  "looks": null
}
```

`notices`, `skipped`, `review` and `failed` are lists of `{line, text, problem}` in the order the planner appended them (not sorted).
Every string is compared exactly, so the prose of `reasons` and `problem` is part of the contract.

## 5. `expected/review.json`

`versevision.workflow.report.build_report(plan, source_name="input.<ext>")`: the structure the review page reads.

```jsonc
{ "source": "input.txt",
  "summary": { "slides", "verse_slides", "point_slides", "flagged", "not_added", "needs_review", "left_out", "adjustments",
               "errors", "fixes", "warnings", "good" },                      // all integers
  "looks": null,
  "slides":    [ {"n": 1, "kind": "point" | "verse", "title": "...", "text": "..."} ],   // n is 1-based; point title = first line of the text
  "decisions": [ {"id", "line", "kind", "title", "text", "tier", "reasons",
                  "slides": [8, 9, 10],        // 1-based slide numbers; [] when slide_count is 0
                  "addable": true,             // add_text is set and the decision has no slide
                  "add_text": "...",
                  "correction": null | {"from": "...", "to": "..."}} ],   // original_slides is not in the report
  "review": { "error": [decision...], "warning": [...], "good": [...],              // by tier, left-out lines excluded
              "left_out_warning": [...], "left_out_good": [...] },                    // left-out lines by tier
  "flagged":     [ {"line", "text", "detail", "status": "not_added" | "needs_review" | "needs_approval"} ],   // sorted by line (stable)
  "adjustments": [ {"line", "text", "detail", "status": "adjusted"} ],
  "left_out":    [ {"line", "text", "detail", "status": "left_out"} ],
  "clean": true }                                                                    // no flagged lines
```

The web-only extras (`lines`, `mode`, `title`, `library`, `target`, `node`) are added by the server, not by `build_report`; they are not in the oracle.

## 6. `expected/trace.jsonl`

One JSON object per line, in the order the Python app wrote them: `{"seq": 1, "stage": "extract", ...fields}`. `seq` counts from 1 with no gaps,
so a missing or extra record shows up as a different `seq` further down. Strings longer than 2000 characters are cut to 2000 and
`... [N more characters]` is appended (N = the number of characters removed); lists keep their first 300 items; a key containing `api_key`, `apikey`,
`authorization`, `password`, `secret`, `token` has the value `"[REDACTED]"`.

The stages, in the order they appear, with the S-stage that has to produce them in Rust (`PLAN.md`):

| stage | when | fields | Rust stage |
|---|---|---|---|
| `extract` | once, first | `type` (".txt"), `method`, `characters`, `lines`, `blank_lines`, `odd_characters` (sorted non-letter characters above U+007F, at most 40) | S4 |
| `parse_start` | once | `lines`, `non_blank`, `full_line_width`, `hard_wrapped`, `wrapped_lines`, `orphan_max_words`, `note` | S2 |
| `line_join` | a wrapped line joined to the line above | `line`, `into_paragraph`, `text`, `why` | S2 |
| `paragraph` | every paragraph of the document | `line`, `end_line`, `first_in_document`, `label`, `marker`, `raw_lines`, `references`, `rule`, `text`, `why` | S2 |
| `item` | every parsed item | `line`, `label`, `settings`, `outcome` (a list of objects; the keys that occur are `kind`, `category`, `line`, `reason`, `text`, `label`, `role`, `reference`, `version_written`, `warnings`, depending on the kind of item) | S2 |
| `label_fix` | a point header corrected to match the others | `line`, `written`, `corrected_to`, `rule` | S3 |
| `version` | every verse | `line`, `reference`, `version`, `how`, `note`, `sets_version_for_later_verses` or `version_source`, `default_version`, `last_explicit_version` | S3 |
| `point_format` | every point | `line`, `role`, `label_in_notes`, `text_in_notes`, `slide_text`, `slides`, `settings` | S3 |
| `provider` | every Bible lookup | see section 1 (written by the offline provider) | oracle runner |
| `quote_check` | a verse quoted in the notes without a version | `line`, `reference`, `quoted`, `version`, `score`, `tried`, `result` | S3 |
| `fetch_result` | every verse that came back | `line`, `reference`, `version_asked`, `version_returned`, `verses`, `characters`, `first_verse`, `last_verse`, `copyright`, `note` | S3 |
| `split` | every verse that came back | `line`, `reference`, `slides`, `settings`, `verses` (per verse: `verse`, `characters`, `wrapped_lines`, `slides`, `pieces`), `note` | S3 |
| `decision` | one per decision, after all items | `id`, `line`, `kind`, `tier`, `title`, `reasons`, `slides` (1-based) | S3 |
| `plan` | once, last | `slide_count`, `verse_slides`, `point_slides`, `slides` (per slide: `n`, `kind`, `reference`, `version`, `text`, `verses`) | S3 |

The order in a run is: `extract`, `parse_start`, then the `paragraph` records (each preceded by its `line_join` records when it joined wrapped lines),
then one `item` record per parsed item, then every `label_fix`, then every `version` record (all verses are resolved before any is fetched), then,
entry by entry in document order, `point_format` for a point or `provider`, `quote_check` (only when the quote check ran), `fetch_result`, `split`
for a verse, then every `decision`, then `plan`. The authoritative order is the one in the files.

## 7. `choices.json` and `plan_after_choices.json`

`apply_review_choices(result, skip=set(...), include=set(...), reject_fixes=set(...))` with the decision ids of `choices.json`. The expected file is
the resulting `SlidePlan` (`{"slides": [...], "attribution_slides": []}`), in the slide shape of section 4. The `review_*` trace records this call
would add are not part of the oracle.

## 8. Line-level oracles

One JSON object per line; `args` hold every parameter by its Python name, so a runner needs no defaults.

* `oracle/references.jsonl`: `{"input": "<one line>", "aliases": null | {"YLT": "NLT"}, "references": [...], "issues": [...]}` from
  `parse_references(input, aliases)`. A `reference` is the `PassageReference` dump (`book_id`, `book_display_name`, `spans` (`start`/`end`
  as `{chapter, verse}`, `end` may be null), `version`, `source_text`, `source_start`, `source_end`, `normalized_text`, `warnings`); an issue is
  `{source_text, source_start, source_end, message, suggestion}`. Offsets are Python string indexes (code points, not bytes). A few inputs make
  the Python parser *raise* (chapter 0 or verse 0); those lines have `"raises": "<exception name>"` and empty lists; see "Known Python quirks".
* `oracle/fitting.jsonl`: `{"fn", "args", "out"}` for `balance_lines`, `split_verse_text`, `split_point_text` (the three the planner uses) and the two
  measuring helpers under them: `text_width_units(value) -> float`, `wrap_body_lines(text, line_width_units) -> list[str]`. `out` is the exact result.
* `oracle/versions.jsonl`: `{"kind": "version", "input", "out": {"canonical", "corrected", "is_real"}}` for `canonical_version`, `correct_version`
  and `is_real_version`; `{"kind": "version_available", "input", "available", "out": {"corrected"}}` for `correct_version(input, set(available))`;
  `{"kind": "quote_score", "quoted", "verse_texts", "out"}` for `quote_score` (the share of the quoted words found in order in the verses; a float).
  `quote_score` comes from `difflib.SequenceMatcher(autojunk=False)`: a port must give the same matching blocks (compare the float with a
  tolerance of 1e-12).

* `oracle/config_defaults.json`, `config_defaults.toml`, `config_rich.toml`, `config_options.jsonl`: the config model. The JSON is `AppConfig().model_dump(mode="json")`.
  The TOML files are what `save_config` writes (`model_dump(mode="json", exclude_none=True)` through `tomli_w`); Rust must write the same bytes (a config
  with the same values has one text), and `config_rich.toml` read and written again must not change. `config_options.jsonl` rows are
  `{"base": <dict for AppConfig.model_validate>, "profile": null | "name", "sets": ["a.b=c", ...], "out": {"changed": {"a.b": value, ...}, "removed": [...]} | {"error": "..."}}`
  from `with_options`; `changed` lists the dotted paths of the leaf fields that differ from the base (a list or an empty table is one leaf), and an
  `error` is the exact `ConfigurationError` text, including pydantic's message for the first invalid field in model order. Unknown keys in a TOML file are
  ignored (pydantic's default); an unknown name in `--set` is an error.

## 9. How a Rust run is compared

* A case passes when `plan.json`, `review.json` and `trace.jsonl` of the Rust run **equal the expected files field by field**: same keys, same
  values, same list order, same number of lines. The only tolerances are **key order inside an object** (compare parsed JSON, not bytes) and
  float formatting (`48` equals `48.0`; compare as numbers).
* Text is compared exactly, character for character: no trimming, no case folding, no Unicode normalisation, no collapsing of spaces. A line break
  inside `body_text` is a `\n`, and a missing or extra one is a failure. The prose in `reasons`, `problem`, `why`, `rule` and `note` is part of the
  contract.
* The first difference is reported as a JSON path (`decisions[7].reasons[0]`, `trace[41].settings.max_body_lines`) with both values.
* A field present in the Rust output but not in the expected file is a failure too (the contract is closed).
* `provider` trace records are written by the oracle runner's own offline provider, so they equal the file by construction; the runner must still
  write them (they take a `seq` number).
* Do not "fix" an expectation to make Rust pass. If Rust is right and Python is wrong, the PR shows the Python bug and the owner decides; the
  rule is below.

## 10. Rule for changes

**Every Python rule change adds a case and regenerates the oracle** (`cd python && uv run python scripts/export_oracle.py`), in the same pull
request. A case that shows the new behaviour is added to `export_oracle.py` first; the regenerated `oracle/` is committed with it; the diff of
`oracle/` is the review of the behaviour change. The Rust port must match the new oracle before the next release of either. No oracle file is
edited by hand.

## Known Python quirks the oracle records (not fixed here)

* `parse_references` raises a pydantic `ValidationError` for a chapter or verse of `0` in a position it builds a `VersePoint`
  (`John 0:1` and `John 1:0` are in the oracle). The planner never sees these as references; the oracle records the line with `"raises"`. A Rust parser should
  return an issue instead, and S2 should say so in its PR (an intentional, listed difference).
