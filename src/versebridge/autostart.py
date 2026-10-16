"""Start `versebridge serve` automatically when the user logs in.

macOS: a LaunchAgent. Linux: a systemd user service. Windows: a Task Scheduler logon task.
Only the current user is affected; nothing needs administrator rights, and `disable` removes it.
"""
from __future__ import annotations

import os
import plistlib
import shutil
import subprocess
import sys
from pathlib import Path

from pydantic import BaseModel

from versebridge.errors import ConfigurationError

LABEL = "org.versebridge.serve"
SYSTEMD_UNIT = "versebridge-serve.service"
WINDOWS_TASK = "VerseBridge"


class AutostartStatus(BaseModel):
    supported: bool
    enabled: bool
    platform: str
    detail: str = ""
    host: str | None = None
    port: int | None = None
    file: str | None = None


def platform_name() -> str:
    if sys.platform == "darwin":
        return "macos"
    if sys.platform.startswith("win"):
        return "windows"
    return "linux"


def program_args(host: str, port: int) -> list[str]:
    exe = shutil.which("versebridge")
    base = [exe] if exe else [sys.executable, "-m", "versebridge.cli"]
    return [*base, "serve", "--host", host, "--port", str(port), "--no-open"]


# --- file builders (pure, unit-tested) ---------------------------------------
def launch_agent_plist(args: list[str], log_path: str) -> bytes:
    return plistlib.dumps({
        "Label": LABEL,
        "ProgramArguments": args,
        "RunAtLoad": True,
        "KeepAlive": {"SuccessfulExit": False},
        "StandardOutPath": log_path,
        "StandardErrorPath": log_path,
        "ProcessType": "Background",
        "EnvironmentVariables": {"PATH": "/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:" + str(Path.home() / ".local/bin")},
    })


def _systemd_quote(arg: str) -> str:
    return '"' + arg.replace("\\", "\\\\").replace('"', '\\"') + '"' if any(c in arg for c in ' \t"\\') else arg


def systemd_unit(args: list[str]) -> str:
    return (
        "[Unit]\nDescription=VerseBridge web page\nAfter=network-online.target\n\n"
        f"[Service]\nExecStart={' '.join(_systemd_quote(a) for a in args)}\nRestart=on-failure\nRestartSec=5\n\n"
        "[Install]\nWantedBy=default.target\n"
    )


def windows_task_command(args: list[str]) -> str:
    return " ".join(f'"{a}"' if " " in a else a for a in args)


# --- paths -------------------------------------------------------------------
def _launch_agent_path() -> Path:
    return Path.home() / "Library" / "LaunchAgents" / f"{LABEL}.plist"


def _systemd_path() -> Path:
    base = Path(os.environ.get("XDG_CONFIG_HOME") or Path.home() / ".config")
    return base / "systemd" / "user" / SYSTEMD_UNIT


def _log_path() -> Path:
    from platformdirs import user_log_path
    folder = user_log_path("versebridge")
    folder.mkdir(parents=True, exist_ok=True)
    return folder / "serve.log"


def _run(command: list[str], *, check: bool = True) -> subprocess.CompletedProcess:
    result = subprocess.run(command, capture_output=True, text=True, timeout=30)
    if check and result.returncode != 0:
        raise ConfigurationError(f"{' '.join(command[:3])} failed: {(result.stderr or result.stdout).strip()[-300:]}")
    return result


# --- actions -----------------------------------------------------------------
def status() -> AutostartStatus:
    plat = platform_name()
    if plat == "macos":
        path = _launch_agent_path()
        info = plistlib.loads(path.read_bytes()) if path.exists() else {}
        return _with_args(AutostartStatus(supported=True, enabled=path.exists(), platform=plat, file=str(path)), info.get("ProgramArguments"))
    if plat == "linux":
        path = _systemd_path()
        enabled = path.exists() and _run(["systemctl", "--user", "is-enabled", SYSTEMD_UNIT], check=False).returncode == 0
        args = None
        if path.exists():
            line = next((l for l in path.read_text().splitlines() if l.startswith("ExecStart=")), "")
            args = line.removeprefix("ExecStart=").replace('"', "").split()
        return _with_args(AutostartStatus(supported=shutil.which("systemctl") is not None, enabled=enabled, platform=plat, file=str(path)), args)
    result = _run(["schtasks", "/query", "/tn", WINDOWS_TASK], check=False)
    return AutostartStatus(supported=True, enabled=result.returncode == 0, platform=plat)


def _with_args(state: AutostartStatus, args) -> AutostartStatus:
    if args and "--port" in args:
        state.port = int(args[args.index("--port") + 1])
    if args and "--host" in args:
        state.host = args[args.index("--host") + 1]
    return state


def enable(host: str = "127.0.0.1", port: int = 8777) -> AutostartStatus:
    if not (1 <= port <= 65535):
        raise ConfigurationError("port must be between 1 and 65535")
    if host not in {"127.0.0.1", "localhost", "::1"}:
        raise ConfigurationError("Autostart only supports the local address (127.0.0.1); a network address needs a private access token each start")
    plat = platform_name()
    args = program_args(host, port)
    if plat == "macos":
        path = _launch_agent_path()
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(launch_agent_plist(args, str(_log_path())))
        domain = f"gui/{os.getuid()}"
        _run(["launchctl", "bootout", domain, str(path)], check=False)
        _run(["launchctl", "bootstrap", domain, str(path)])
    elif plat == "linux":
        if not shutil.which("systemctl"):
            raise ConfigurationError("systemd was not found; autostart is not supported on this system")
        path = _systemd_path()
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(systemd_unit(args), encoding="utf-8")
        _run(["systemctl", "--user", "daemon-reload"])
        _run(["systemctl", "--user", "enable", "--now", SYSTEMD_UNIT])
    else:
        _run(["schtasks", "/create", "/f", "/sc", "onlogon", "/tn", WINDOWS_TASK, "/tr", windows_task_command(args)])
    return status()


def disable() -> AutostartStatus:
    plat = platform_name()
    if plat == "macos":
        path = _launch_agent_path()
        if path.exists():
            _run(["launchctl", "bootout", f"gui/{os.getuid()}", str(path)], check=False)
            path.unlink()
    elif plat == "linux":
        path = _systemd_path()
        if path.exists():
            _run(["systemctl", "--user", "disable", "--now", SYSTEMD_UNIT], check=False)
            path.unlink()
            _run(["systemctl", "--user", "daemon-reload"], check=False)
    else:
        _run(["schtasks", "/delete", "/f", "/tn", WINDOWS_TASK], check=False)
    return status()
