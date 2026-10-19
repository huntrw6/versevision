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
WHISPER_DIR = TOOLS / "whisper" / "whisper.cpp"
WHISPER_SERVER = WHISPER_DIR / "build2" / "bin" / "whisper-server"
WHISPER_PORT = 8178
# Telling Whisper which words to expect makes it spell books and numbers the way Scripture does.
PROMPT = ("Sermon. Bible references: Genesis, Exodus, Psalm, Proverbs, Isaiah, Jeremiah, Habakkuk, Zephaniah, Matthew, Mark, Luke, John, Acts, "
          "Romans, 1 Corinthians, 2 Corinthians, Galatians, Ephesians, Philippians, Colossians, 1 Thessalonians, 2 Timothy, Philemon, Hebrews, "
          "James, 1 Peter, Jude, Revelation. John 3:16. Romans 8:28.")


def _log_dir() -> Path:
    from platformdirs import user_log_path
    d = user_log_path("versevision") / "listen"
    d.mkdir(parents=True, exist_ok=True)
    return d


def listen_command(
    device: Annotated[str, typer.Option("--device", help="Part of the input's name (default: Dante)")] = "Dante",
    dry_run: Annotated[bool, typer.Option("--dry-run", help="Only log what it would show; put nothing on screen")] = False,
    hold: Annotated[float, typer.Option("--hold", help="Seconds a verse stays on screen after the speaker last touched it")] = 30.0,
    presentation: Annotated[str, typer.Option("--presentation", help="Add every verse it shows to this presentation")] = "Verses",
    no_add: Annotated[bool, typer.Option("--no-add", help="Do not add shown verses to a presentation")] = False,
    follow: Annotated[bool, typer.Option("--follow/--no-follow", help="Move on to the next verse when the speaker starts reading it")] = True,
    content: Annotated[bool, typer.Option("--content/--no-content", help="Recognise a verse from its words when no reference was said")] = True,
    cooldown: Annotated[float, typer.Option("--cooldown", help="Do not show the same verse again within this many seconds")] = 30.0,
    settle: Annotated[float, typer.Option("--settle", help="Seconds a reference must stay unchanged before it is acted on")] = 0.8,
    seconds: Annotated[float, typer.Option("--seconds", help="Stop after this many seconds (0 = until stopped)")] = 0.0,
    engine: Annotated[str, typer.Option("--engine", help="whisper (default, more accurate) or apple")] = "whisper",
    model: Annotated[str, typer.Option("--model", help="Whisper model: base.en, small.en (default) or medium.en")] = "small.en",
    debug: Annotated[bool, typer.Option("--debug", help="Keep each cut of audio the listener sends to Whisper (in the run's log folder)")] = False,
) -> None:
    """Listen to the sermon and put the verses on screen. Ctrl-C stops it (and clears the screen).

    By default it listens on Dante, shows each verse for 30 seconds after the speaker last touches it, and adds every shown verse
    to the "Verses" presentation. It acts when the speaker says a reference, follows along as a passage is read, and recognises a
    verse from its words when the reference was never said.
    """
    from versevision.follow import Action, Follower
    from versevision.scripture import ScriptureIndex, default_dir, download

    if not APP.exists():
        typer.secho(f"The listener app is missing: {APP}", fg="red"); raise typer.Exit(2)
    index = ScriptureIndex.load()
    if (content or follow) and not index:
        try:
            typer.echo("Fetching the Bible text used to recognise verses (once) ...")
            download(log=typer.echo)
            index = ScriptureIndex.load()
        except Exception as exc:
            typer.secho(f"Could not fetch the Bible text ({exc}). Continuing with spoken references only.", fg="yellow")
            content = follow = False
    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    run_dir = _log_dir() / stamp
    run_dir.mkdir(parents=True)
    raw = run_dir / "heard.jsonl"
    out_log = run_dir / "detections.log"

    def log(line: str) -> None:
        typer.echo(line)
        with open(out_log, "a", encoding="utf-8") as fh:
            fh.write(line + "\n")

    started_server: subprocess.Popen | None = None
    if engine == "whisper":
        started_server = _ensure_whisper_server(model)
    args = ["open", "-n", str(APP), "--args", "--out", str(raw), "--engine", engine]
    if engine == "whisper": args += ["--whisper", f"http://127.0.0.1:{WHISPER_PORT}", "--prompt", PROMPT, "--silence", "0.9"]
    if debug: args += ["--debug-dir", str(run_dir / "audio")]
    if device: args += ["--device", device]
    if seconds: args += ["--seconds", str(seconds)]
    stop_allow = _auto_allow()
    subprocess.run(args, check=True)
    where = presentation if (not no_add and not dry_run) else "nowhere"
    log(f"Listening on '{device or 'the default input'}' with {engine}{' ' + model if engine == 'whisper' else ''}. "
        f"{'DRY RUN, nothing is shown.' if dry_run else f'Verses stay up {hold:.0f} s and are added to: {where}.'}  Log: {run_dir}")

    follower = Follower(index=index if (content or follow) else None, hold=hold, cooldown=cooldown, follow=follow, content=content)
    follower.detector.settle = settle
    added: set[str] = set()
    target = {"uuid": ""}
    if not dry_run and not no_add:
        target["uuid"] = _select_presentation(presentation, log)

    def clear_screen() -> None:
        try:
            import httpx
            httpx.get("http://127.0.0.1:1025/v1/clear/layer/slide", timeout=3)
        except Exception as exc:
            log(f"            could not clear the screen: {exc}")

    def perform(act: Action) -> None:
        if act.kind == "clear":
            log(f"[{_stamp(act.t)}] clear           {act.reason}")
            if not dry_run:
                clear_screen()
            return
        log(f"[{_stamp(act.t)}] {'would show' if dry_run else 'show      '}      {act.reference:22} because {act.reason}; heard: {act.heard!r}")
        if dry_run:
            return
        cmd = [str(VVADD), "--keep", _lookup_text(act.reference)]
        if target["uuid"] and act.reference not in added:
            cmd.insert(1, "--add")
        t0 = time.time()
        try:
            res = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
            ok = "live after" in res.stdout
            if ok and "--add" in cmd:
                added.add(act.reference)
            log(f"            {'on screen' if ok else 'FAILED'} in {time.time() - t0:.2f}s: {res.stdout.strip().splitlines()[-1][:110] if res.stdout.strip() else res.stderr[-100:]}")
        except Exception as exc:                           # never let a display problem stop the listening
            log(f"            show failed for {act.reference}: {exc}")

    deadline = time.time() + seconds + 3 if seconds else None
    try:
        for _ in range(100):
            if raw.exists() and raw.stat().st_size > 0: break
            time.sleep(0.1)
        for item in _until(listen_mod.read_lines(raw, follow=True), deadline):
            if item.get("kind") == "status" and ("permission" in item.get("text", "") or "error" in item.get("text", "") or "no input" in item.get("text", "")):
                log(f"listener: {item['text']}")
            for act in follower.feed(item):
                perform(act)
    except KeyboardInterrupt:
        pass
    finally:
        stop_allow()
        if follower.current and not dry_run:
            clear_screen()
        subprocess.run(["pkill", "-f", "VerseVision Listen.app/Contents/MacOS/vvlisten"], capture_output=True)
        if started_server is not None:
            started_server.terminate()
    typer.echo(f"Stopped. Log: {run_dir}")


def _stamp(t: float) -> str:
    return time.strftime("%H:%M:%S", time.localtime(t)) if t > 1e9 else f"{t:8.2f}s"


def _select_presentation(name: str, log) -> str:
    """Select the named presentation so ProPresenter's "Copy to Selected Presentation" adds the verses to it."""
    try:
        import httpx
        libs = httpx.get("http://127.0.0.1:1025/v1/libraries", timeout=5).json()
        for lib in libs:
            items = httpx.get(f"http://127.0.0.1:1025/v1/library/{lib['uuid']}", timeout=5).json().get("items", [])
            for item in items:
                if item["name"] == name:
                    httpx.get(f"http://127.0.0.1:1025/v1/presentation/{item['uuid']}/focus", timeout=5)
                    log(f"Verses will be added to '{name}'.")
                    return item["uuid"]
        log(f"Could not find a presentation named '{name}', so shown verses will not be added to one.")
    except Exception as exc:
        log(f"Could not select '{name}' ({exc}); shown verses will not be added to a presentation.")
    return ""


def _lookup_text(reference: str) -> str:
    """ProPresenter's lookup box wants "Psalm 23:1", not the printed name "Psalms"."""
    return reference.replace("Psalms ", "Psalm ", 1)


def _auto_allow(seconds: float = 25.0):
    """macOS asks again for Microphone access whenever the listener app is rebuilt. For the first few seconds after
    starting, press Allow on a prompt that names "VerseVision Listen" (and only that). Returns a function that stops it."""
    import threading
    stop = threading.Event()
    helper = TOOLS / "vvdialog"

    def work() -> None:
        end = time.time() + seconds
        while not stop.is_set() and time.time() < end and helper.exists():
            for proc in ("UserNotificationCenter", "tccd", "System Preferences"):
                try:
                    out = subprocess.run([str(helper), proc, "VerseVision Listen", "Allow"], capture_output=True, text=True, timeout=5).stdout
                except Exception:
                    continue
                if out.strip().startswith("PRESSED"):
                    typer.echo("Allowed the macOS permission prompt for VerseVision Listen.")
                    return
            stop.wait(0.5)

    threading.Thread(target=work, daemon=True).start()
    return stop.set


def _server_up() -> bool:
    import httpx
    try:
        return httpx.get(f"http://127.0.0.1:{WHISPER_PORT}/", timeout=1.0).status_code < 500
    except Exception:
        return False


def _ensure_whisper_server(model: str) -> "subprocess.Popen | None":
    """Start the local Whisper server if it is not already running. Returns the process we started (to stop later)."""
    if _server_up():
        return None
    model_file = WHISPER_DIR / "models" / f"ggml-{model}.bin"
    if not WHISPER_SERVER.exists() or not model_file.exists():
        typer.secho(f"Whisper is not installed ({WHISPER_SERVER} / {model_file.name}). Use --engine apple, or run the Whisper setup.", fg="red")
        raise typer.Exit(2)
    proc = subprocess.Popen([str(WHISPER_SERVER), "-m", str(model_file), "-t", "6", "--port", str(WHISPER_PORT), "--host", "127.0.0.1"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env={**os.environ, "DYLD_LIBRARY_PATH": str(WHISPER_DIR / "build2" / "bin")})
    for _ in range(100):
        if _server_up(): break
        time.sleep(0.2)
    else:
        proc.terminate(); typer.secho("The Whisper server did not start.", fg="red"); raise typer.Exit(2)
    _warm_up()
    typer.echo(f"Whisper {model} ready.")
    return proc


def _warm_up() -> None:
    """The first request to a fresh server is slow (it loads buffers). Send a second of silence so a real sentence is not."""
    import httpx, io, wave
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(16000); w.writeframes(b"\x00\x00" * 16000)
    try:
        httpx.post(f"http://127.0.0.1:{WHISPER_PORT}/inference", files={"file": ("w.wav", buf.getvalue(), "audio/wav")},
                   data={"temperature": "0.0", "response_format": "json"}, timeout=30)
    except Exception:
        pass


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
            yield {"kind": "status", "t": _now_t(), "w": time.time(), "text": "tick"}


_T0 = time.time()
def _now_t() -> float:
    return time.time() - _T0
