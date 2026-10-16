"""Generate the minimal Python binding closure needed for Presentation files."""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "proto" / "source" / "propresenter19"
OUTPUT = ROOT / "src" / "versevision" / "presentation" / "generated"
IMPORT = re.compile(r'^import\s+"([^"]+)"\s*;', re.MULTILINE)


def dependency_closure(entrypoint: str) -> list[str]:
    pending = [entrypoint]
    found: set[str] = set()
    while pending:
        relative = pending.pop()
        if relative in found or relative.startswith("google/protobuf/"):
            continue
        path = SOURCE / relative
        if not path.is_file():
            raise FileNotFoundError(f"missing imported schema: {relative}")
        found.add(relative)
        pending.extend(IMPORT.findall(path.read_text(encoding="utf-8")))
    return sorted(found)


def main() -> None:
    OUTPUT.mkdir(parents=True, exist_ok=True)
    files = dependency_closure("presentation.proto")
    command = [
        sys.executable,
        "-m",
        "grpc_tools.protoc",
        f"-I{SOURCE}",
        f"--python_out={OUTPUT}",
        f"--pyi_out={OUTPUT}",
        *files,
    ]
    subprocess.run(command, check=True, cwd=SOURCE)
    (OUTPUT / "__init__.py").touch()
    print(f"Generated {len(files)} schema modules in {OUTPUT}")


if __name__ == "__main__":
    main()

