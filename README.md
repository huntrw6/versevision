# VerseVision

VerseVision turns sermon notes (txt, md, html, docx, pdf) into ProPresenter Bible-verse and sermon-point slides. It is written in Rust, ships inside StagePilot, and also runs on its own (`versevision serve` opens the page on your computer).

| Where | What |
|---|---|
| `crates/` | The Rust workspace (`rust-toolchain.toml` pins the compiler). `versevision-core` is the library (config model, line fitting, plan types, references, versions, the notes classifier and the planner), `versevision-bible` the Bible providers (API.Bible, YouVersion, ESV, local USFM files, the StagePilot Bible service chosen by `bible.source = auto | stagepilot | own_keys`, the key store and the scripture cache) and `versevision` the command line (`--version` only for now). `cargo test --workspace` includes the oracle runner, `cargo test -p versevision-core --test oracle -- --nocapture` prints how much of the oracle passes. |
| `oracle/` | What the former Python app (git tag `python-final`) produced for a corpus of documents (plans, review reports, decision traces) and for single lines (references, line fitting, versions). The Rust port must reproduce it exactly. Regenerate with `cd python && uv run python scripts/export_oracle.py`; see `oracle/README.md`. |
| `docs/PLAN_CONTRACT.md` | The exact shape of the plan, review and trace files, and how a Rust run is compared with the oracle. |
| `proto/` | The ProPresenter protobuf schemas, shared by both implementations. |

Rule: a rule change updates the matching oracle case in the same pull request.
