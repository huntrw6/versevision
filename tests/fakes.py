"""A fake ProPresenter node backed by local files, to exercise apply/replace/undo/rollback safely."""
import re
import shutil
from pathlib import Path

from versebridge.node.transport import sha256_file
from versebridge.presentation.generated import presentation_pb2


class FakeNode:
    description = "fake node"

    name = "Sample"

    def __init__(self, store: Path, presentation: Path, *, show_wrong_text: bool = False):
        self.store, self.file, self.running, self.log = store, presentation, True, []
        self.show_wrong_text = show_wrong_text      # simulate ProPresenter not showing the new slides
        store.mkdir(parents=True, exist_ok=True)

    def _slides(self):
        m = presentation_pb2.Presentation()
        m.ParseFromString(self.file.read_bytes())
        out = []
        for c in m.cues:
            for a in c.actions:
                if a.WhichOneof("ActionTypeData") == "slide":
                    parts = []
                    for e in a.slide.presentation.base_slide.elements:
                        if e.element.HasField("text"):
                            raw = e.element.text.rtf_data.decode("latin1")
                            tail = raw.split("strokec3")[-1].split("\\up0")[-1]
                            tail = re.sub(r"\\u(-?\d+)\?", lambda x: chr(int(x.group(1)) % 65536), tail)
                            parts.append(re.sub(r"\\[a-z]+-?\d* ?|[{}]", "", tail).replace("\\", "").strip())
                    out.append({"text": "\r".join(parts)})
        return out

    def api_get(self, route):
        if route == "/version":
            return {"host_description": "fake", "api_version": "v1"}
        if route == "/v1/libraries":
            return [{"uuid": "L1", "name": "Verses"}]
        if route == "/v1/library/L1":
            return {"items": [{"uuid": "P1", "name": self.name}]}
        if route == "/v1/presentation/P1":
            slides = self._slides()
            if self.show_wrong_text and self.running:
                slides = slides[:3]
            return {"presentation": {"id": {"uuid": "P1", "name": self.name}, "presentation_path": str(self.file),
                                       "groups": [{"name": "", "slides": slides}]}}
        raise KeyError(route)

    def is_running(self):
        return self.running

    def quit_app(self):
        self.running = False
        self.log.append("quit")

    def launch_app(self):
        self.running = True
        self.log.append("launch")

    def fetch_file(self, remote, local):
        shutil.copy2(remote, local)
        return sha256_file(local)

    def install_file(self, local, remote, expected):
        assert not self.running, "app must be closed"
        assert sha256_file(Path(remote)) == expected, "changed under us"
        shutil.copy2(local, remote)
        self.log.append("install")
        return sha256_file(Path(remote))

    def backup_file(self, remote):
        b = self.store / f"{Path(remote).name}.bak{len(list(self.store.iterdir()))}"
        shutil.copy2(remote, b)
        return str(b)

    def restore_backup(self, backup, remote):
        assert not self.running, "app must be closed"
        shutil.copy2(backup, remote)
        self.log.append("restore")


def _find(self, name, library):
    return [str(self.file)] if name == self.name else []


FakeNode.find_presentation = _find
