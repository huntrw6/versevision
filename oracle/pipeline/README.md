# `oracle/pipeline/`: the ProPresenter side (S6b)

What the Python app does when it talks to ProPresenter: the questions it asks (every request, in order, at which second), the files it writes,
renames and deletes, the history, the run folder and the audit log. Written by `python/scripts/oracle_pipeline.py` (called from
`export_oracle.py`), read by `crates/versevision-pp/tests/pipeline_oracle.rs`; `python/tests/test_pipeline_oracle.py` runs every scenario again and
checks the committed files are exactly what comes out.

## How a scenario runs

* **Time is virtual.** `time.sleep` only moves a clock; `time.monotonic`, `datetime.now` and the run log's `elapsed` follow it. A 90 second playlist
  convergence takes no time, and every request carries the virtual second it was made at (`t`).
* **Ids are seeded.** The n-th id is the version-4 uuid made from `sha256("<seed>:<n>")` (the writer's `new_uuid` and the pipeline's `uuid4` draw from
  one sequence; so does the run log's `os.urandom`). The seed is the scenario's folder name. Nothing in the Python app changed for this: the oracle
  replaces the module attributes (`pipeline.time`, `pipeline.uuid`, `protobuf_writer.new_uuid`, `runlog.os`, ...) while it runs.
* **ProPresenter is a scripted fake HTTP server** (`FakePP` in `oracle_pipeline.py`, `tests/fake_pp/mod.rs` in Rust: the same rules).
* **The Bible text is made up**, recorded into `expected/bible.json` and served back by both sides.
* **The files** are real: the library folders are copies of the fixtures `tests/fixtures/{sample,propresenter19,themes}` and
  `oracle/pro/files`, in a scratch folder (`<ROOT>` in the evidence).

## The fake ProPresenter

All times are virtual seconds since the scenario started. `scenario.json` -> `server`:

| key | meaning |
| --- | --- |
| `landing` | what happens to a file that lands (is created or renamed into a library folder): `listed_after` (s until the library lists it), `blank_for` (s the listing shows a blank id), `reissue_at` (list of s at which it gets another fresh id), `loaded_after` (s until `GET /v1/presentation/<id>` answers 200 for the current id; `null` = never), `prefix` (of the ids, default `N`) |
| `rename_landing`, `by_name` | the same keys, for a file that lands by rename, and per presentation name |
| `put_status` | status of `PUT /v1/playlist/<id>` (default 204; anything else is refused with `{"error": "refused"}`) |
| `put_needs_loaded` | a PUT that names a presentation id that is not a loaded current id answers 400 |
| `put_new_item_ids` | every playlist item gets a new id on each save (what ProPresenter does) |
| `presentation_404` | ids that always answer 404 |
| `show_slides` | `GET /v1/presentation/<id>` shows only the first n slides |
| `api_version` | what `/version` says (default `v1`) |

A presentation's slides are read from the file on disk (the text of every slide's text elements, as `_plain_rtf` gives it; the cue's name is the label).
Steps can change the server (`{"op": "server", "set": {...}}`), the process (`running`), the clock (`advance`), or put a file in the library (`write_file`).
`hooks` overwrite a file right after the pipeline's `create_file` / `rename_file` (what ProPresenter re-saving a file does to the renamed original).

## What is compared

For each scenario `expected/` holds (Rust must produce the same, after `<ROOT>` and `<PORT>` are substituted):

| file | content |
| --- | --- |
| `result.json` | `steps` (what each step answered: the exit code and what `--output json` printed; for `explain`/`replay` the text), `requests` (virtual second, method, path, body, status of every HTTP call), `file_ops` (every call of the pipeline to the transport, with its result or error), `files` (sha256 of every file left in the libraries), `running`, `playlist` |
| `history.json` | `imports.json` byte for byte (Windows: compared as JSON, the path separators differ) |
| `audit.jsonl` | the audit log |
| `runs/<n>/` | every file of every run folder (`events.jsonl`, `trace.jsonl`, `options.json`, `report.json`, `source.txt`, `summary.txt`) and its folder name |
| `config.toml`, `bible.json` | the config the run used and the verse text served (inputs the Rust test reads) |

Not compared: the `traceback` text of a failed run (it holds the machine's paths and the source's line numbers; the oracle keeps `"<traceback>"`).
Rust records the exception's name and message there.

`selection.jsonl` is `rapidfuzz`'s answer (`fuzz.WRatio` and `process.extract`) for the near-miss hints of "Presentation target was not found".

## Scenarios

See `scenarios/*/scenario.json` (the `description` of each says what it shows). A step is one of: `run`, `undo`, `history`, `log_list`, `explain`,
`replay`, `client` (the typed API client and the presentation selection), `web_plan` / `web_apply` (the review page's two phases), `advance`,
`running`, `server`, `write_file`, `seed_runs`.

## Adding one

Write `scenarios/<name>/scenario.json`, run `python scripts/oracle_pipeline.py <name>` from `python/`, read `expected/result.json` (is it what
the Python app should do?), commit. The scenario's name is its seed: renaming it changes every id.
