"""`versevision listen`: hear the sermon, spot Bible references, optionally show them on screen for N seconds.

Dry run is the default: it only writes a log. Add --show to put each reference on the output for --hold seconds.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path
from typing import Annotated

import typer

from versevision import listen as listen_mod

TOOLS = Path.home() / "VerseVision-tools"
APP = TOOLS / "VerseVision Listen.app"
VVADD = TOOLS / "vvadd"


def _log_dir() -> Path:
    from platformdirs import user_log_path
    d = user_log_path("versevision") / "listen"
    d.mkdir(parents=True, exist_ok=True)
    return d


def listen_command(
    device: Annotated[str, typer.Option("--device", help="Part of the input's name, e.g. 'Dante' or 'iMac Pro Microphone'")] = "",
    show: Annotated[bool, typer.Option("--show", help="Put each reference on screen (default is a dry run that only logs)")] = False,
    hold: Annotated[float, typer.Option("--hold", help="Seconds to keep a verse on screen")] = 10.0,
    cooldown: Annotated[float, typer.Option("--cooldown", help="Do not show the same reference again within this many seconds")] = 30.0,
    settle: Annotated[float, typer.Option("--settle", help="Seconds a reference must stay unchanged before it is acted on")] = 0.6,
    seconds: Annotated[float, typer.Option("--seconds", help="Stop after this many seconds (0 = until stopped)")] = 0.0,
    file: Annotated[str, typer.Option("--file", help="Recognise an audio file instead of a live input (testing)")] = "",
) -> None:
    """Listen for spoken Bible references. Ctrl-C stops it."""
    if not APP.exists():
        typer.secho(f"The listener app is missing: {APP}", fg="red"); raise typer.Exit(2)
    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    run_dir = _log_dir() / stamp
    run_dir.mkdir(parents=True)
    raw = run_dir / "heard.jsonl"
    out_log = run_dir / "detections.log"
    args = ["open", "-n", str(APP), "--args", "--out", str(raw)]
    if device: args += ["--device", device]
    if file: args += ["--file", file]
    if seconds: args += ["--seconds", str(seconds)]
    subprocess.run(args, check=True)
    typer.echo(f"Listening{' (will show on screen)' if show else ' (dry run: nothing is shown)'}. Log: {run_dir}")

    def log(line: str) -> None:
        typer.echo(line)
        with open(out_log, "a", encoding="utf-8") as fh:
            fh.write(line + "\n")

    shown_lock = {"busy": False}

    def put_on_screen(reference: str) -> bool:
        if shown_lock["busy"]:
            return False                                   # one verse at a time; a second waits for the next mention
        shown_lock["busy"] = True
        t0 = time.time()
        try:
            res = subprocess.run([str(VVADD), f"--show={hold}", reference], capture_output=True, text=True, timeout=hold + 15)
            ok = "live after" in res.stdout and "cleared" in res.stdout
            log(f"            shown {reference} in {(time.time() - t0) - hold:.2f}s over the hold: {'ok' if ok else 'FAILED ' + res.stdout[-120:]}")
            return ok
        except Exception as exc:                           # never let a display problem stop the listening
            log(f"            show failed for {reference}: {exc}")
            return False
        finally:
            shown_lock["busy"] = False

    detector = listen_mod.Detector(settle=settle, cooldown=cooldown)
    deadline = time.time() + seconds + 3 if seconds else None
    try:
        # wait for the listener to create its file
        for _ in range(100):
            if raw.exists() and raw.stat().st_size > 0: break
            time.sleep(0.1)
        lines = listen_mod.read_lines(raw, follow=True)
        for item in _until(lines, deadline):
            if item.get("kind") == "status" and ("permission" in item.get("text", "") or "error" in item.get("text", "")):
                log(f"listener: {item['text']}")
            listen_mod.run([item], show=put_on_screen if show else None, detector=detector, log=log)
    except KeyboardInterrupt:
        pass
    finally:
        subprocess.run(["pkill", "-f", "VerseVision Listen.app/Contents/MacOS/vvlisten"], capture_output=True)
    typer.echo(f"Stopped. Log: {run_dir}")


def _until(lines, deadline):
    """Yield from the file reader, but stop at the deadline even if the file goes quiet."""
    import itertools, threading, queue
    q: "queue.Queue" = queue.Queue()
    def pump():
        for item in lines:
            q.put(item)
    threading.Thread(target=pump, daemon=True).start()
    while True:
        try:
            yield q.get(timeout=0.2)
        except queue.Empty:
            if deadline and time.time() > deadline: return
            yield {"kind": "status", "t": _now_t(), "text": "tick"}


_T0 = time.time()
def _now_t() -> float:
    return time.time() - _T0
