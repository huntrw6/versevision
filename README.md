# VerseVision

VerseVision turns sermon notes (txt, md, html, docx, pdf) into ProPresenter Bible-verse and sermon-point slides. It is being **rewritten in Rust in this
repository** and will ship inside StagePilot; the Python app is the specification until the Rust one matches it.

| Where | What |
|---|---|
| `python/` | The Python app (version 0.38.x): source, tests, build and install scripts, its own README. Run its tests with `cd python && uv run --extra dev pytest -q -p no:cacheprovider`. It keeps getting releases until the Rust app takes over. |
| `oracle/` | What the Python app produces for a corpus of documents (plans, review reports, decision traces) and for single lines (references, line fitting, versions). The Rust port must reproduce it exactly. Regenerate with `cd python && uv run python scripts/export_oracle.py`; see `oracle/README.md`. |
| `docs/PLAN_CONTRACT.md` | The exact shape of the plan, review and trace files, and how a Rust run is compared with the oracle. |
| `proto/` | The ProPresenter protobuf schemas, shared by both implementations. |

Rule: every Python rule change adds a case to the oracle and regenerates it in the same pull request.
