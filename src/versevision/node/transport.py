"""Where ProPresenter runs: this computer, or another one reached through a remote-access tool.

A transport answers six questions: what does the ProPresenter API say, is the app running,
close/open it, and move the presentation file safely (checksummed) to and from this computer.
All file moves are verified by SHA-256 and refuse to overwrite a file that changed since it was read.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Protocol

import httpx

from versevision.config import NodeConfig, ProPresenterConfig
from versevision.errors import ConfigurationError, ProPresenterConnectionError, PresentationWriteError


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


class Transport(Protocol):
    description: str

    def api_get(self, route: str) -> Any: ...
    def api_put(self, route: str, body: Any) -> None: ...
    def is_running(self) -> bool: ...
    def quit_app(self) -> None: ...
    def launch_app(self) -> None: ...
    def fetch_file(self, remote_path: str, local_path: Path) -> str: ...
    def install_file(self, local_path: Path, remote_path: str, expected_remote_sha: str) -> str: ...
    def find_presentation(self, name: str, library: str | None) -> list[str]: ...
    def library_folder(self, library: str) -> str: ...
    def create_file(self, local_path: Path, remote_path: str) -> str: ...
    def delete_created_file(self, remote_path: str, expected_cues: int) -> str: ...
    def backup_file(self, remote_path: str) -> str: ...
    def restore_backup(self, backup_path: str, remote_path: str) -> None: ...


class LocalTransport:
    """ProPresenter runs on this computer."""

    def __init__(self, pp: ProPresenterConfig, node: NodeConfig):
        self.pp, self.node = pp, node
        self.description = f"this computer ({pp.host}:{pp.port})"

    def api_get(self, route: str) -> Any:
        try:
            response = httpx.get(f"http://{self.pp.host}:{self.pp.port}{route}", timeout=self.pp.timeout_seconds)
            response.raise_for_status()
            return response.json()
        except httpx.HTTPError as exc:
            raise ProPresenterConnectionError(f"Could not reach ProPresenter at {self.pp.host}:{self.pp.port}: {exc}") from exc

    def api_put(self, route: str, body: Any) -> None:
        try:
            response = httpx.put(f"http://{self.pp.host}:{self.pp.port}{route}", json=body, timeout=self.pp.timeout_seconds + 5)
            response.raise_for_status()
        except httpx.HTTPError as exc:
            raise ProPresenterConnectionError(f"ProPresenter did not accept the change to {route}: {exc}") from exc

    def is_running(self) -> bool:
        from versevision.propresenter.process import is_propresenter_running
        return is_propresenter_running()

    @staticmethod
    def _sh(command: str, timeout: float = 60.0) -> str:
        result = subprocess.run(["/bin/sh", "-c", command], capture_output=True, text=True, timeout=timeout)
        return (result.stdout or "") + (result.stderr or "")

    @property
    def _app_name(self) -> str:
        return Path(self.node.app_path).stem

    def _ensure_helper(self) -> str | None:
        helper_dir = os.path.expandvars(self.node.helper_dir)
        helper = os.path.join(helper_dir, "vvdialog")
        if os.access(helper, os.X_OK):
            return shlex.quote(helper)
        source = Path(__file__).with_name("vvdialog.swift")
        if sys.platform != "darwin" or not source.exists():
            return None
        os.makedirs(helper_dir, exist_ok=True)
        self._sh(f"swiftc -O -o {shlex.quote(helper)} {shlex.quote(str(source))}", timeout=240)
        return shlex.quote(helper) if os.access(helper, os.X_OK) else None

    def quit_app(self) -> None:
        """Close ProPresenter. Its "Are you sure you want to quit?" box is answered; "Save changes?" never is."""
        if sys.platform != "darwin":
            raise ConfigurationError("Automatic quit is only available on macOS; close ProPresenter yourself")
        app = shlex.quote(self._app_name)
        subprocess.Popen(["osascript", "-e", f'tell application "{self._app_name}" to quit'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if not self.node.confirm_quit:
            return
        helper = self._ensure_helper()
        if not helper:
            return
        question, name = shlex.quote("Are you sure you want to quit"), shlex.quote(self.node.process_name)
        for _ in range(15):
            out = self._sh(f"{helper} {name} {question} OK")
            if "PRESSED" in out or "NOT_RUNNING" in out:
                return
            time.sleep(1.0)
        # a scripted quit can be cancelled by the app ("User canceled"); a real Cmd-Q goes through the menu path
        self._sh(f"osascript -e 'tell application {app} to activate'; sleep 1; "
                 f"(osascript -e 'tell application \"System Events\" to keystroke \"q\" using command down' >/dev/null 2>&1 &); "
                 f"for i in 1 2 3 4 5 6 7 8 9 10; do r=$({helper} {name} {question} OK); case \"$r\" in PRESSED|NOT_RUNNING) break;; esac; sleep 0.5; done")

    def launch_app(self) -> None:
        if sys.platform != "darwin":
            raise ConfigurationError("Automatic launch is only available on macOS; reopen ProPresenter yourself")
        subprocess.run(["open", self.node.app_path], check=False)

    def fetch_file(self, remote_path: str, local_path: Path) -> str:
        local_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(remote_path, local_path)
        return sha256_file(local_path)

    def install_file(self, local_path: Path, remote_path: str, expected_remote_sha: str) -> str:
        target = Path(remote_path)
        if sha256_file(target) != expected_remote_sha:
            raise PresentationWriteError("The presentation changed after VerseVision read it; no files changed")
        temp = None
        try:
            with tempfile.NamedTemporaryFile(prefix=f".{target.name}.versevision-", suffix=".tmp", dir=target.parent, delete=False) as stream:
                temp = Path(stream.name)
                stream.write(local_path.read_bytes())
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temp, target)
            temp = None
        finally:
            if temp and temp.exists():
                temp.unlink()
        return sha256_file(target)

    def library_folder(self, library: str) -> str:
        return str(Path(os.path.expandvars(self.node.library_root)).expanduser() / library)

    def delete_created_file(self, remote_path: str, expected_cues: int) -> str:
        from versevision.presentation.generated import presentation_pb2
        target = Path(remote_path)
        if not target.is_file():
            return "The presentation file was already gone"
        message = presentation_pb2.Presentation()
        message.ParseFromString(target.read_bytes())
        if len(message.cues) != expected_cues:
            raise PresentationWriteError(
                f"{target.name} has {len(message.cues)} slides now, not the {expected_cues} VerseVision added, so it was edited. "
                "It was not deleted; remove it in ProPresenter if you no longer want it"
            )
        target.unlink()
        return "Presentation removed"

    def create_file(self, local_path: Path, remote_path: str) -> str:
        """Copy a new file into place. Never replaces an existing file."""
        target = Path(remote_path)
        if target.exists():
            raise PresentationWriteError(f"{target.name} already exists; nothing was changed")
        temp = None
        try:
            with tempfile.NamedTemporaryFile(prefix=f".{target.name}.versevision-", suffix=".tmp", dir=target.parent, delete=False) as stream:
                temp = Path(stream.name)
                stream.write(local_path.read_bytes())
                stream.flush()
                os.fsync(stream.fileno())
            os.link(temp, target)
        finally:
            if temp and temp.exists():
                temp.unlink()
        return sha256_file(target)

    def find_presentation(self, name: str, library: str | None) -> list[str]:
        root = Path(os.path.expandvars(self.node.library_root)).expanduser()
        if not root.exists():
            return []
        folders = [root / library] if library else [item for item in root.iterdir() if item.is_dir()]
        return sorted(str(folder / f"{name}.pro") for folder in folders if (folder / f"{name}.pro").is_file())

    def backup_file(self, remote_path: str) -> str:
        from versevision.presentation.backup import create_backup
        return str(create_backup(Path(remote_path)))

    def restore_backup(self, backup_path: str, remote_path: str) -> None:
        from versevision.presentation.backup import restore_backup
        restore_backup(Path(remote_path), Path(backup_path))


class HermesRemoteTransport:
    """ProPresenter runs on another Mac enrolled with the `hermes-remote` tool.

    File access to ~/Documents needs the permissioned helper app (`hermes-remote gui`), so every
    file operation goes through it; API calls use plain `run`.
    """

    def __init__(self, pp: ProPresenterConfig, node: NodeConfig):
        if not node.name:
            raise ConfigurationError("A node name is required (--node NAME or [node] name in the configuration)")
        if not shutil.which(node.command):
            raise ConfigurationError(f"{node.command!r} was not found on this computer")
        self.pp, self.node = pp, node
        self.name = node.name
        self.description = f"{node.name} via {node.command}"
        self._app_name = Path(node.app_path).stem
        self._port = pp.port

    # --- plumbing ---------------------------------------------------------
    def _invoke(self, args: list[str], timeout: float) -> subprocess.CompletedProcess:
        last: subprocess.CompletedProcess | None = None
        for attempt in range(self.node.retries + 1):
            last = subprocess.run([self.node.command, *args], capture_output=True, text=True, timeout=timeout)
            combined = (last.stdout or "") + (last.stderr or "")
            if "helper app did not answer" not in combined and "unreachable" not in combined.lower():
                return last
            if attempt < self.node.retries:
                time.sleep(self.node.retry_delay_seconds)
        assert last is not None
        return last

    def _sh(self, command: str, *, gui: bool, timeout: float = 90.0) -> tuple[int, str]:
        wrapped = f"{command}\nrc=$?\necho\necho \"__VB_RC=$rc\""
        if gui:
            result = self._invoke(["gui", "--sh", self.name, wrapped], timeout)
        else:
            result = self._invoke(["run", self.name, "--", "/bin/sh", "-c", wrapped], timeout)
        text = ((result.stdout or "") + (result.stderr or "")).replace("\r", "")
        match = list(re.finditer(r"__VB_RC=(\d+)", text))
        if not match:
            raise ProPresenterConnectionError(f"{self.node.command} did not complete on {self.name}: {text.strip()[-300:] or 'no output'}")
        last = match[-1]
        return int(last.group(1)), text[:last.start()].strip()

    @staticmethod
    def _q(value: str) -> str:
        return shlex.quote(value)

    def _remote_tmp(self, suffix: str = ".pro") -> str:
        return f"/tmp/versevision-{uuid.uuid4().hex[:12]}{suffix}"

    # --- Transport --------------------------------------------------------
    def api_get(self, route: str) -> Any:
        code, body = self._sh(f"curl -s -m {int(self.pp.timeout_seconds) + 5} http://127.0.0.1:{self._port}{route}", gui=False)
        if code != 0 or not body:
            raise ProPresenterConnectionError(
                f"ProPresenter API on {self.name} did not answer for {route}. Is ProPresenter running with its Network API enabled?"
            )
        try:
            return json.loads(body)
        except ValueError as exc:
            raise ProPresenterConnectionError(f"Malformed ProPresenter response for {route} from {self.name}") from exc

    def api_put(self, route: str, body: Any) -> None:
        payload = json.dumps(body)
        if "VVEOF" in payload:
            raise ProPresenterConnectionError("Unsupported characters in the playlist data")
        code, out = self._sh(
            f"curl -s -m {int(self.pp.timeout_seconds) + 10} -o /dev/null -w '%{{http_code}}' -X PUT "
            f"-H 'Content-Type: application/json' --data-binary @- http://127.0.0.1:{self._port}{route} <<'VVEOF'\n{payload}\nVVEOF",
            gui=False,
        )
        if code != 0 or not out.strip().endswith(("200", "204")):
            raise ProPresenterConnectionError(f"ProPresenter on {self.name} did not accept the change to {route} (answer: {out.strip()[-40:] or 'none'})")

    def is_running(self) -> bool:
        code, _ = self._sh(f"pgrep -f {self._q(self.node.process_pattern)} >/dev/null", gui=False)
        return code == 0

    def _ensure_helper(self) -> str | None:
        """Compile the small dialog helper on the node once. Returns its path, or None if unavailable."""
        helper = f'"{self.node.helper_dir}/vvdialog"'
        code, out = self._sh(f"[ -x {helper} ] && echo ready", gui=True)
        if "ready" in out:
            return helper
        source = Path(__file__).with_name("vvdialog.swift")
        if not source.exists():
            return None
        tmp = self._remote_tmp(".swift")
        pushed = self._invoke(["push", self.name, str(source), tmp], 120)
        if pushed.returncode != 0:
            return None
        code, out = self._sh(
            f'mkdir -p "{self.node.helper_dir}" && swiftc -O -o {helper} {self._q(tmp)} 2>&1 | tail -3; rm -f {self._q(tmp)}; [ -x {helper} ] && echo ready',
            gui=True, timeout=240,
        )
        return helper if "ready" in out else None

    def quit_app(self) -> None:
        """Ask the app to quit. If it shows its "Are you sure you want to quit?" box, press OK.

        Only that one box is answered. A "Save changes?" box is never touched: that choice belongs to a person.
        """
        script = f'tell application "{self._app_name}" to quit'
        self._sh(f"nohup osascript -e {self._q(script)} >/dev/null 2>&1 &\nsleep 1", gui=True, timeout=30)
        if not self.node.confirm_quit:
            return
        helper = self._ensure_helper()
        if not helper:
            return
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            code, out = self._sh(f"{helper} {self._q(self.node.process_name)} {self._q('Are you sure you want to quit')} OK", gui=True, timeout=30)
            if "PRESSED" in out or "NOT_RUNNING" in out:
                return
            time.sleep(1.0)
        # The scripted quit can be cancelled by the app itself ("User canceled"); a real Cmd-Q goes through
        # the normal menu path and is answered by the same helper.
        self._sh(
            f'osascript -e {self._q(f"tell application \"{self._app_name}\" to activate")}; sleep 1; '
            f'(osascript -e {self._q("tell application \"System Events\" to keystroke \"q\" using command down")} >/dev/null 2>&1 &); '
            f'for i in 1 2 3 4 5 6 7 8 9 10; do r=$({helper} {self._q(self.node.process_name)} {self._q("Are you sure you want to quit")} OK); '
            f'case "$r" in PRESSED|NOT_RUNNING) break;; esac; sleep 0.5; done',
            gui=True, timeout=60,
        )

    def launch_app(self) -> None:
        code, out = self._sh(f"open {self._q(self.node.app_path)}", gui=True)
        if code != 0:
            raise ProPresenterConnectionError(f"Could not open ProPresenter on {self.name}: {out[-200:]}")

    def fetch_file(self, remote_path: str, local_path: Path) -> str:
        tmp = self._remote_tmp()
        code, out = self._sh(f"cp {self._q(remote_path)} {self._q(tmp)} && shasum -a 256 {self._q(tmp)}", gui=True)
        if code != 0 or not out:
            raise ProPresenterConnectionError(f"Could not read {remote_path} on {self.name}: {out[-200:]}")
        remote_sha = out.split()[0]
        try:
            local_path.parent.mkdir(parents=True, exist_ok=True)
            pulled = self._invoke(["pull", self.name, tmp, str(local_path)], 120)
            if pulled.returncode != 0 or not local_path.exists():
                raise ProPresenterConnectionError(f"Transfer from {self.name} failed: {(pulled.stdout + pulled.stderr).strip()[-200:]}")
        finally:
            self._sh(f"rm -f {self._q(tmp)}", gui=False)
        local_sha = sha256_file(local_path)
        if local_sha != remote_sha:
            raise ProPresenterConnectionError("Transferred file did not match the original checksum; nothing was changed")
        return local_sha

    def install_file(self, local_path: Path, remote_path: str, expected_remote_sha: str) -> str:
        tmp = self._remote_tmp()
        local_sha = sha256_file(local_path)
        pushed = self._invoke(["push", self.name, str(local_path), tmp], 120)
        if pushed.returncode != 0:
            raise PresentationWriteError(f"Transfer to {self.name} failed: {(pushed.stdout + pushed.stderr).strip()[-200:]}")
        try:
            code, out = self._sh(
                f"[ \"$(shasum -a 256 < {self._q(tmp)} | cut -d' ' -f1)\" = {local_sha} ] || {{ echo STAGED_MISMATCH; exit 5; }}\n"
                f"pgrep -f {self._q(self.node.process_pattern)} >/dev/null && {{ echo APP_RUNNING; exit 4; }}\n"
                f"[ \"$(shasum -a 256 < {self._q(remote_path)} | cut -d' ' -f1)\" = {expected_remote_sha} ] || {{ echo CHANGED; exit 3; }}\n"
                f"cp {self._q(tmp)} {self._q(remote_path)} && shasum -a 256 {self._q(remote_path)}",
                gui=True,
            )
        finally:
            self._sh(f"rm -f {self._q(tmp)}", gui=False)
        if code == 4:
            raise PresentationWriteError("ProPresenter is running on the node; close it completely before writing. No files changed")
        if code == 3:
            raise PresentationWriteError("The presentation changed after VerseVision read it; no files changed")
        if code != 0 or not out:
            raise PresentationWriteError(f"Install failed on {self.name} ({out[-200:] or 'no output'}); no files changed")
        installed = out.split()[0]
        if installed != local_sha:
            raise PresentationWriteError("Installed file did not match the staged file; restore the backup")
        return installed

    def library_folder(self, library: str) -> str:
        return f"{self.node.library_root}/{library}"

    def delete_created_file(self, remote_path: str, expected_cues: int) -> str:
        # verify the file is still the one VerseVision made (same slide count) before deleting it
        fetch_to = Path(tempfile.mkdtemp(prefix="versevision-")) / "check.pro"
        code, out = self._sh(f'[ -f "{remote_path}" ] && echo present || echo gone', gui=True)
        if out.strip().endswith("gone"):
            return "The presentation file was already gone"
        self.fetch_file(remote_path, fetch_to)
        from versevision.presentation.generated import presentation_pb2
        message = presentation_pb2.Presentation()
        message.ParseFromString(fetch_to.read_bytes())
        if len(message.cues) != expected_cues:
            raise PresentationWriteError(
                f"{Path(remote_path).name} has {len(message.cues)} slides now, not the {expected_cues} VerseVision added, so it was edited. "
                "It was not deleted; remove it in ProPresenter if you no longer want it"
            )
        code, out = self._sh(f'rm -f "{remote_path}" && echo removed', gui=True)
        if "removed" not in out:
            raise PresentationWriteError(f"Could not remove {Path(remote_path).name} on {self.name}: {out[-200:]}")
        return "Presentation removed"

    def create_file(self, local_path: Path, remote_path: str) -> str:
        """Copy a new file into place on the node. Never replaces an existing file."""
        tmp = self._remote_tmp()
        local_sha = sha256_file(local_path)
        pushed = self._invoke(["push", self.name, str(local_path), tmp], 120)
        if pushed.returncode != 0:
            raise PresentationWriteError(f"Transfer to {self.name} failed: {(pushed.stdout + pushed.stderr).strip()[-200:]}")
        try:
            dest = f'"{remote_path}"'
            code, out = self._sh(
                f"[ \"$(shasum -a 256 < {self._q(tmp)} | cut -d' ' -f1)\" = {local_sha} ] || {{ echo STAGED_MISMATCH; exit 5; }}\n"
                f"[ -e {dest} ] && {{ echo EXISTS; exit 3; }}\n"
                f"cp {self._q(tmp)} {dest} && shasum -a 256 {dest}",
                gui=True,
            )
        finally:
            self._sh(f"rm -f {self._q(tmp)}", gui=False)
        if code == 3:
            raise PresentationWriteError(f"{Path(remote_path).name} already exists on {self.name}; nothing was changed")
        if code != 0 or not out or out.split()[0] != local_sha:
            raise PresentationWriteError(f"Could not place the new presentation on {self.name} ({out[-200:] or 'no output'})")
        return out.split()[0]

    def find_presentation(self, name: str, library: str | None) -> list[str]:
        folder = shlex.quote(library) if library else "*"
        # $HOME in library_root must expand on the remote side, so only the variable parts are quoted
        command = f'for f in "{self.node.library_root}"/{folder}/{shlex.quote(name + ".pro")}; do [ -f "$f" ] && echo "$f"; done'
        code, out = self._sh(command, gui=True)
        return [line.strip() for line in out.splitlines() if line.strip().endswith(".pro")]

    def backup_file(self, remote_path: str) -> str:
        stamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")
        name = Path(remote_path).name
        # $HOME (and other variables) in backup_dir must expand on the remote side, so the folder is double-quoted
        folder = f'"{self.node.backup_dir}"'
        code, out = self._sh(
            f"mkdir -p {folder} && cp -p {self._q(remote_path)} {folder}/{self._q(f'{name}.versevision-{stamp}')} "
            f"&& echo {folder}/{self._q(f'{name}.versevision-{stamp}')}",
            gui=True,
        )
        if code != 0 or not out:
            raise PresentationWriteError(f"Could not back up {remote_path} on {self.name}: {out[-200:]}")
        return out.splitlines()[-1].strip()

    def restore_backup(self, backup_path: str, remote_path: str) -> None:
        code, out = self._sh(
            f"pgrep -f {self._q(self.node.process_pattern)} >/dev/null && {{ echo APP_RUNNING; exit 4; }}\n"
            f"cp -p {self._q(backup_path)} {self._q(remote_path)}",
            gui=True,
        )
        if code != 0:
            raise PresentationWriteError(f"Restore failed ({out[-200:]}); restore {backup_path} over {remote_path} by hand")


def make_transport(pp: ProPresenterConfig, node: NodeConfig) -> Transport:
    if node.type == "hermes-remote":
        return HermesRemoteTransport(pp, node)
    return LocalTransport(pp, node)
