#!/usr/bin/env python3
"""Which lines and branches of notes/classifier.py do the oracle documents reach?

    cd python && uv run --frozen --extra dev python scripts/classifier_coverage.py                    # the whole corpus
    cd python && uv run --frozen --extra dev python scripts/classifier_coverage.py --without-group classifier   # the corpus before the classifier-* cases

Every case document is read like the exporter reads it and classified with the case's own options (the same call the planner makes). The
report is the coverage of classifier.py alone, with the lines and branches that no document reaches.
"""
from __future__ import annotations

import argparse
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import tomllib  # noqa: E402

import coverage  # noqa: E402
import tomli_w  # noqa: E402


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--without-group", action="append", default=[], help="leave out the cases of this group (their name starts with GROUP-)")
    args = parser.parse_args()

    target = str(HERE.parent / "src" / "versevision" / "notes" / "classifier.py")
    cov = coverage.Coverage(branch=True, include=[target], data_file=None)
    cov.start()
    import export_oracle as ex                      # noqa: E402   (imported under coverage so module-level lines count)
    from versevision.notes import classifier
    from versevision.documents import extract_text  # noqa: E402

    cases = [c for c in ex.all_cases() if c.name.split("-")[0] not in args.without_group]
    used = 0
    for case in cases:
        options_toml = tomli_w.dumps(ex.sort_tree({**case.options, **({"run": case.run} if case.run else {})}))
        parsed = tomllib.loads(options_toml)
        config = ex.build_config({k: v for k, v in parsed.items() if k != "run"})
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / f"input.{case.ext}"
            path.write_bytes(case.data)
            text = extract_text(path)
        classifier.classify_document(text, config.bible.version_aliases, config.notes, config.points)
        used += 1
    cov.stop()
    print(f"documents classified: {used}")
    cov.report(include=[target], show_missing=True)


if __name__ == "__main__":
    main()
