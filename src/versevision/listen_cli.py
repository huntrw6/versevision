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
          "James, 1 Peter, Jude, Revelation.")


def _log_dir() -> Path:
    from platformdirs import user_log_path
    d = user_log_path("versevision") / "listen"
    d.mkdir(parents=True, exist_ok=True)
    return d


def listen_command(
    device: Annotated[str, typer.Option("--device", help="Part of the input's name (default: Dante)")] = "Dante",
    dry_run: Annotated[bool, typer.Option("--dry-run", help="Only log what it would show; put nothing on screen")] = False,
    hold: Annotated[float, typer.Option("--hold", help="Seconds a verse stays on screen after the speaker last touched it")] = 30.0,
    presentation: Annotated[str, typer.Option("--presentation", help="Add every verse to this named presentation. Default: whichever presentation is open (selected) in ProPresenter at that moment")] = "",
    no_add: Annotated[bool, typer.Option("--no-add", help="Do not add shown verses to a presentation")] = False,
    preload: Annotated[int, typer.Option("--preload", help="Quietly load this many verses ahead into the presentation, so reading on is instant (0 = off)")] = 4,
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
    to the presentation that is open in ProPresenter. It acts when the speaker says a reference, follows along as a passage is read, and recognises a
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
    if index:
        typer.echo("Verse wording loaded for recognition: " + ", ".join(sorted(index.bibles)))
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
    where = (presentation or "the presentation open in ProPresenter") if (not no_add and not dry_run) else "nowhere"
    log(f"Listening on '{device or 'the default input'}' with {engine}{' ' + model if engine == 'whisper' else ''}. "
        f"{'DRY RUN, nothing is shown.' if dry_run else f'Verses stay up {hold:.0f} s and are added to: {where}.'}  Log: {run_dir}")

    follower = Follower(index=index if (content or follow) else None, hold=hold, cooldown=cooldown, follow=follow, content=content)
    follower.detector.settle = settle
    added: set[str] = set()
    target = {"uuid": ""}
    if not dry_run and not no_add and presentation:
        target["uuid"] = _select_presentation(presentation, log)         # a fixed, named target; otherwise the open one is used each time

    def clear_screen() -> None:
        try:
            import httpx
            httpx.get("http://127.0.0.1:1025/v1/clear/layer/slide", timeout=3)
        except Exception as exc:
            log(f"            could not clear the screen: {exc}")

    import threading
    from versevision.slidemap import SlideMap
    ui_lock = threading.Lock()                       # one piece of ProPresenter automation at a time (lookups use the keyboard and mouse)
    maps: dict[str, SlideMap] = {}                   # presentation uuid -> which verses are slides there and at what index
    state = {"preloading": False, "version": ""}     # the Bible version last put on screen (the one a slide is preferred in)

    def refresh_map(uuid: str) -> SlideMap:
        """Re-read a presentation's slides and rebuild the verse map from their text (about 80 ms)."""
        maps[uuid.upper()] = SlideMap.from_texts(uuid, slide_texts(uuid))
        return maps[uuid.upper()]

    def current_target() -> str:
        if no_add:
            return ""
        return target["uuid"] if presentation else focused_uuid()

    def trigger_slide(uuid: str, idx: int) -> bool:
        """Put a slide of the presentation on screen through the API and confirm ProPresenter reports it live."""
        import httpx
        try:
            httpx.get(f"{PP}/v1/presentation/{uuid}/{idx}/trigger", timeout=3).raise_for_status()
            end = time.time() + 1.2
            while time.time() < end:
                cur = (_pp_get("/v1/presentation/slide_index") or {}).get("presentation_index") or {}
                if cur.get("index") == idx and (cur.get("presentation_id") or {}).get("uuid", "").upper() == uuid.upper():
                    return True
                time.sleep(0.02)
        except Exception as exc:
            log(f"            could not trigger slide {idx}: {exc}")
        return False

    def preload_after(act: Action) -> None:
        """After a verse is on screen, quietly add the next few verses of the chapter to the presentation (one lookup, one slide per
        verse) so that reading on is a 0.1 s trigger instead of a 1.3 s lookup. Only runs when fewer than half are already there."""
        if preload <= 0 or dry_run or no_add or index is None or state["preloading"]:
            return
        uuid = current_target()
        known = maps.get(uuid.upper()) if uuid else None
        if not uuid or known is None:
            return
        ahead = [act.verse + k for k in range(1, preload + 1) if index.has_verse(act.book_id, act.chapter, act.verse + k)]
        have, missing = known.ahead(act.book_id, act.chapter, act.verse, preload, pp_short(act.version), state["version"])
        if not missing or len(have) >= max(2, len(ahead) // 2):
            return
        state["preloading"] = True

        def work() -> None:
            try:
                with ui_lock:
                    first, last = missing[0], missing[-1]
                    text = _lookup_text(_range_text(act.book, act.chapter, first, (act.chapter, last) if last > first else None))
                    before = slide_count(uuid)
                    prev = focused_uuid()
                    if prev.upper() != uuid.upper():
                        _pp_get(f"/v1/presentation/{uuid}/focus"); time.sleep(0.15)
                    t0 = time.time()
                    want_version = pp_version(act.version)
                    cmd = [str(VVADD), "--back"] + ([f"--version={want_version}"] if want_version else []) + [text]   # no --keep: add only
                    try:
                        subprocess.run(cmd, capture_output=True, text=True, timeout=30)
                        end = time.time() + 1.5
                        while slide_count(uuid) <= before and time.time() < end:
                            time.sleep(0.05)
                        m2 = refresh_map(uuid)
                        got = sum(1 for v in range(first, last + 1) if m2.find(act.book_id, act.chapter, v, pp_short(act.version), state["version"]))
                        log(f"            preloaded {got}/{last - first + 1} verses after {act.reference} ({text}) in {time.time() - t0:.2f}s; "
                            f"{len(m2.verses)} verse slides are now in the presentation")
                    finally:
                        if prev and prev.upper() != uuid.upper():
                            try: _pp_get(f"/v1/presentation/{prev}/focus")
                            except Exception: pass
            except Exception as exc:
                log(f"            preload failed: {exc}")
            finally:
                state["preloading"] = False

        threading.Thread(target=work, daemon=True).start()

    def focused_uuid() -> str:
        try:
            return (_pp_get("/v1/presentation/focused") or {}).get("uuid", "") or ""
        except Exception:
            return ""

    def verify_in_background(uuid: str, act: Action, idx: int) -> None:
        """The map can go stale if the operator edits the presentation. After the verse is already on screen, re-read the slides and, if the
        slide that was triggered is not this verse, correct the screen."""
        def work() -> None:
            try:
                m2 = refresh_map(uuid)
                e = m2.find(act.book_id, act.chapter, act.verse, pp_short(act.version), state["version"])
                if e is None:
                    log(f"            (check) {act.reference} is no longer a slide in the presentation")
                elif e.index != idx:
                    log(f"            (check) the slide moved from {idx} to {e.index}; showing the right one")
                    trigger_slide(uuid, e.index)
            except Exception as exc:
                log(f"            (check) failed: {exc}")
        threading.Thread(target=work, daemon=True).start()

    def perform(act: Action) -> None:
        if act.kind == "clear":
            log(f"[{_stamp(act.t)}] clear           {act.reason}")
            if not dry_run:
                clear_screen()
            return
        flag = (" (a guess: checked against the words that follow)" if act.reason.startswith("assumed") else " (tentative: earliest of several equally good matches)") if act.tentative else ""
        log(f"[{_stamp(act.t)}] {'would show' if dry_run else 'show      '}      {act.reference:22} because {act.reason}; heard: {act.heard!r}{flag}")
        if dry_run:
            return
        t0 = time.time()
        uuid = current_target()
        want_version = pp_version(act.version)
        # 1. Is this verse already a slide? Then no lookup: trigger it.
        if uuid and not (act.late_version and False):
            known = maps.get(uuid.upper())
            if known is None:
                known = refresh_map(uuid)
            entry = known.find(act.book_id, act.chapter, act.verse, pp_short(act.version), state["version"])
            if entry is not None:
                if trigger_slide(uuid, entry.index):
                    log(f"            on screen in {time.time() - t0:.2f}s: slide {entry.index} of {len(known.verses)} verse slides, no lookup ({entry.version})")
                    if entry.version:
                        state["version"] = entry.version
                    verify_in_background(uuid, act, entry.index)
                    preload_after(act)
                    return
                log("            the loaded slide did not come up; looking it up instead")
        # 2. Look it up (and add it, so the next time it is a slide).
        if not no_add and not presentation:
            target["uuid"] = focused_uuid()                  # whichever presentation the operator has open right now
            uuid = target["uuid"]
            if not uuid:
                log("            no presentation is open, so this verse is shown but not added")
        verses = range_verses(index, act.book_id, act.chapter, act.verse, act.range_end)
        text = _lookup_text(_range_text(act.book, act.chapter, act.verse, act.range_end if len(verses) > 1 else None))
        adding = bool(target["uuid"]) if presentation else bool(uuid)
        cmd = [str(VVADD), "--keep", "--back"] + (["--add"] if adding else []) + ([f"--version={want_version}"] if want_version else []) + [text]
        if act.version:
            log(f"            version {'said' if act.version_source == 'said' else 'recognised from the wording'}: {act.version}"
                + (f" -> selecting '{want_version}'" if want_version else " (not one ProPresenter has; selector left as it was)"))
        prev = focused_uuid() if adding else ""
        if adding and prev.upper() != uuid.upper():
            try:
                _pp_get(f"/v1/presentation/{uuid}/focus")
                time.sleep(0.15)
            except Exception:
                pass
        ok = False
        try:
            with ui_lock:
                res = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
            ok = "live after" in res.stdout
            tail = res.stdout.strip().splitlines()[0][:100] if res.stdout.strip() else res.stderr[-100:]
            log(f"            {'on screen' if ok else 'FAILED'} in {time.time() - t0:.2f}s: {tail}")
            if ok and adding:
                time.sleep(0.1)
                m2 = refresh_map(uuid)
                log(f"            the presentation now has {len(m2.verses)} verse slides")
            if ok:
                state["version"] = (want_version and act.version) or state["version"]
        except Exception as exc:                           # never let a display problem stop the listening
            log(f"            show failed for {act.reference}: {exc}")
        finally:
            if adding and prev and prev.upper() != uuid.upper():
                try:
                    _pp_get(f"/v1/presentation/{prev}/focus")      # give the operator back the presentation they had selected
                except Exception:
                    pass
        if ok:
            preload_after(act)

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


PP = "http://127.0.0.1:1025"

# Names in ProPresenter's Bible-version dropdown. A version not listed here (or not installed) leaves the selector as it was.
PP_VERSION_NAMES = {
    "NIV": "New International Version", "ESV": "English Standard Version", "NLT": "New Living Translation",
    "NKJV": "New King James Version", "KJV": "King James Version", "NASB1995": "New American Standard Bible 1995",
    "NASB": "New American Standard Bible 1995", "AMPC": "Amplified Bible, Classic Edition", "KJ21": "21st Century King James Version",
}


def pp_short(abbreviation: str) -> str:
    """The short name a slide prints for a version ("NIV"), or "" when no version was asked for."""
    a = (abbreviation or "").upper()
    return {"NASB1995": "NASB", "NASB2020": "NASB"}.get(a, a)


def pp_version(abbreviation: str) -> str:
    return PP_VERSION_NAMES.get((abbreviation or "").upper(), "")


def _pp_get(path: str, timeout: float = 3.0):
    import httpx
    r = httpx.get(PP + path, timeout=timeout)
    return r.json() if r.content and r.headers.get("content-type", "").startswith("application/json") else None


def slide_count(uuid: str) -> int:
    try:
        d = _pp_get(f"/v1/presentation/{uuid}")["presentation"]
        return sum(len(g["slides"]) for g in d["groups"])
    except Exception:
        return -1


def slide_texts(uuid: str) -> list[str]:
    """The text of every slide of a presentation, in order (the first line of a Bible slide is "James 1:3 (ESV)")."""
    try:
        d = _pp_get(f"/v1/presentation/{uuid}")["presentation"]
        return [(sl.get("text") or "") for g in d["groups"] for sl in g["slides"]]
    except Exception:
        return []


def slide_holds(text: str, reference: str) -> bool:
    """True when a slide's text begins with this reference ("James 1:3 (ESV) ..."), whatever Bible it is in."""
    ref = _lookup_text(reference).lower()
    head = (text or "").strip().lower()
    return head.startswith(ref + " ") or head.startswith(ref + "\n") or head == ref


def range_verses(index, book_id: str, chapter: int, verse: int, range_end: tuple[int, int] | None) -> list[tuple[int, int]]:
    """The (chapter, verse) pairs a lookup of the named passage produces, in order."""
    if not range_end:
        return [(chapter, verse)]
    out: list[tuple[int, int]] = []
    c, v = chapter, verse
    guard = 0
    while (c, v) <= range_end and guard < 400:
        if index is not None and not index.has_verse(book_id, c, v):
            if index.has_verse(book_id, c + 1, 1) and v > 1:
                c, v = c + 1, 1                     # ran off the end of the chapter
                continue
            break
        out.append((c, v))
        v += 1
        guard += 1
    return out or [(chapter, verse)]


def _range_text(book: str, chapter: int, verse: int, range_end: tuple[int, int] | None) -> str:
    base = f"{book} {chapter}:{verse}"
    if not range_end or range_end == (chapter, verse):
        return base
    rc, rv = range_end
    return f"{base}-{rv}" if rc == chapter else f"{base}-{rc}:{rv}"


def _stamp(t: float) -> str:
    return (time.strftime("%H:%M:%S", time.localtime(t)) + f".{int((t % 1) * 1000):03d}") if t > 1e9 else f"{t:8.2f}s"


def _select_presentation(name: str, log) -> str:
    """Find the presentation verses are added to. It is selected only for the moment of each add, then the operator's own selection is restored."""
    try:
        import httpx
        libs = httpx.get("http://127.0.0.1:1025/v1/libraries", timeout=5).json()
        for lib in libs:
            items = httpx.get(f"http://127.0.0.1:1025/v1/library/{lib['uuid']}", timeout=5).json().get("items", [])
            for item in items:
                if item["name"] == name:
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
    # -t 10: all the iMac Pro's cores; -ac 512: a 10 s attention window instead of 30 s (4x faster, same words for sentences this short)
    proc = subprocess.Popen([str(WHISPER_SERVER), "-m", str(model_file), "-t", str(min(10, max(4, (os.cpu_count() or 8) // 2))), "-ac", "512", "-nf",
                             "--port", str(WHISPER_PORT), "--host", "127.0.0.1"],
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
