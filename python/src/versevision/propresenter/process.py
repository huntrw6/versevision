import psutil


def propresenter_processes() -> list[psutil.Process]:
    matches: list[psutil.Process] = []
    for process in psutil.process_iter(("name", "exe")):
        try:
            name = process.info.get("name") or ""
            if name.casefold() in {"propresenter", "propresenter.exe"}:
                matches.append(process)
        except (psutil.AccessDenied, psutil.NoSuchProcess):
            continue
    return matches


def is_propresenter_running() -> bool:
    return bool(propresenter_processes())

