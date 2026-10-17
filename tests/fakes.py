"""A fake ProPresenter node backed by local files, to exercise apply/replace/undo/rollback safely."""
import re
import shutil
from pathlib import Path

from versevision.node.transport import sha256_file
from versevision.presentation.generated import presentation_pb2


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


def _folder(self, library):
    return str(self.file.parent)


def _create(self, local, remote):
    target = Path(remote)
    if target.exists():
        raise FileExistsError(remote)
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(local, target)
    self.extra = getattr(self, "extra", {})
    self.extra[target.stem] = target
    self.log.append("create")
    return sha256_file(target)


def _delete(self, remote, expected_cues):
    m = presentation_pb2.Presentation()
    m.ParseFromString(Path(remote).read_bytes())
    if len(m.cues) != expected_cues:
        from versevision.errors import PresentationWriteError
        raise PresentationWriteError("edited")
    Path(remote).unlink()
    getattr(self, "extra", {}).pop(Path(remote).stem, None)
    return "Presentation removed"


FakeNode.library_folder = _folder
FakeNode.create_file = _create
FakeNode.delete_created_file = _delete
_orig_api = FakeNode.api_get


def _api(self, route):
    extra = getattr(self, "extra", {})
    if route == "/v1/library/L1":
        return {"items": [{"uuid": "P1", "name": self.name}] + [{"uuid": f"X-{n}", "name": n} for n in extra]}
    if route.startswith("/v1/presentation/X-"):
        name = route.split("X-", 1)[1]
        other = FakeNode(self.store, extra[name])
        other.running = False
        result = other.api_get("/v1/presentation/P1")
        result["presentation"]["id"]["name"] = name
        return result
    return _orig_api(self, route)


FakeNode.api_get = _api


# ---- playlists: one playlist holding the presentation, like a service order
_PL_ID = "PL1"


def _playlist_state(self):
    if not hasattr(self, "playlist"):
        self.playlist = [
            {"id": {"uuid": "H1", "name": "Sermon", "index": 0}, "type": "header", "is_hidden": False, "is_pco": False, "header_color": {"red": .5, "green": .5, "blue": .5, "alpha": 1.0}},
            {"id": {"uuid": "I1", "name": self.name, "index": 1}, "type": "presentation", "is_hidden": False, "is_pco": False,
             "presentation_info": {"presentation_uuid": "P1", "arrangement_name": ""}},
            {"id": {"uuid": "H2", "name": "Closing", "index": 2}, "type": "header", "is_hidden": False, "is_pco": False, "header_color": {"red": .5, "green": .5, "blue": .5, "alpha": 1.0}},
        ]
    return self.playlist


def _api_put(self, route, body):
    if route == f"/v1/playlist/{_PL_ID}":
        import uuid as _u
        for item in body:                                 # like ProPresenter: every save hands out new item AND presentation ids
            item["id"]["uuid"] = str(_u.uuid4()).upper()
            if item.get("presentation_info"):
                item["presentation_info"]["presentation_uuid"] = str(_u.uuid4()).upper()
        self.playlist = body
        self.log.append("playlist-put")
        return
    raise KeyError(route)


_api2_prev = FakeNode.api_get


def _api2(self, route):
    if route == "/v1/playlists":
        return [{"id": {"uuid": _PL_ID, "name": "Sunday", "index": 0}, "field_type": "playlist", "children": []}]
    if route == f"/v1/playlist/{_PL_ID}":
        return {"id": {"uuid": _PL_ID, "name": "Sunday", "index": 0}, "items": _playlist_state(self)}
    return _api2_prev(self, route)


FakeNode.api_put = _api_put
FakeNode.api_get = _api2
