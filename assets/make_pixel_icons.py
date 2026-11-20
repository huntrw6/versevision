"""Hand-drawn 16x16 8-bit sprites for the sidebar: library (orange), playlist (blue), presentation (grey logo mark).
Run: python3 assets/make_pixel_icons.py   -> src/versevision/web/ico-library.svg, ico-playlist.svg, ico-presentation.svg
Each pixel is a <rect> with crisp edges, so the sprites stay hard-edged at any size (24 css px = 3 device pixels per sprite pixel on a retina screen)."""
from pathlib import Path

WEB = Path(__file__).parent.parent / "src" / "versevision" / "web"

LIBRARY = """
..oooooooooooo..
.hhhhhhhhhhhhhh.
oooooooooooooooo
oooooooooooooooo
oooooooowwwwwooo
ooowwoowwwwwwwoo
ooowwooowwwwwwoo
ooowwoooooowwwoo
ooowwoooooowwwoo
ooowwoooooowwwoo
ooowwooowwwwwwoo
ooowwoowwwwwwwoo
oooooooowwwwwooo
oooooooooooooooo
.dddddddddddddd.
..dddddddddddd..
"""
PLAYLIST = """
..bbbbbbbbbbbb..
.hhhhhhhhhhhhhh.
bbbbbbbbbbbbbbbb
bbbbbbbbbbbbbbbb
bbbbbbbbbbbbbbbb
bbbwwwwwbbwbbbbb
bbbbbbbbbbwwbbbb
bbbbbbbbbbwwwbbb
bbbwwwwwbbwwwwbb
bbbbbbbbbbwwwbbb
bbbbbbbbbbwwbbbb
bbbwwwwwbbwbbbbb
bbbbbbbbbbbbbbbb
bbbbbbbbbbbbbbbb
.cccccccccccccc.
..cccccccccccc..
"""
PRESENTATION = """
................
................
................
................
........#####...
...##..#######..
...##...######..
...##......###..
...##......###..
...##......###..
...##...######..
...##..#######..
........#####...
................
................
................
"""


def svg(art, palette):
    rows = [r for r in art.strip("\n").split("\n")]
    assert len(rows) == 16 and all(len(r) == 16 for r in rows), "sprites are 16 x 16"
    rects = []
    for y, row in enumerate(rows):
        x = 0
        while x < 16:
            ch = row[x]
            if ch == ".":
                x += 1
                continue
            end = x
            while end + 1 < 16 and row[end + 1] == ch:
                end += 1
            rects.append(f'<rect x="{x}" y="{y}" width="{end - x + 1}" height="1" fill="{palette[ch]}"/>')
            x = end + 1
    return '<svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg" shape-rendering="crispEdges">' + "".join(rects) + "</svg>\n"


(WEB / "ico-library.svg").write_text(svg(LIBRARY, {"o": "#ee7a22", "d": "#b8520f", "h": "#f9a04a", "w": "#ffffff"}))
(WEB / "ico-playlist.svg").write_text(svg(PLAYLIST, {"b": "#3a85e6", "c": "#2560b8", "h": "#6aa6f2", "w": "#ffffff"}))
(WEB / "ico-presentation.svg").write_text(svg(PRESENTATION, {"#": "#000000"}))      # used as a mask: the page colours it
print("pixel icons written")
