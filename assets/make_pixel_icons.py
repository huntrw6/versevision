"""Hand-drawn 8x8 8-bit sprites for the sidebar: library (orange), playlist (blue), presentation (grey logo mark).
Run: python3 assets/make_pixel_icons.py   -> src/versevision/web/ico-library.svg, ico-playlist.svg, ico-presentation.svg
Each pixel is a <rect> with crisp edges, so the sprites stay hard-edged at any size (24 css px = 6 device pixels per sprite pixel on a retina screen)."""
from pathlib import Path

WEB = Path(__file__).parent.parent / "src" / "versevision" / "web"

LIBRARY = """
.hhhhhh.
ooowwwoo
owowoowo
owowoowo
owowoowo
ooowwwoo
dddddddd
.dddddd.
"""
PLAYLIST = """
.hhhhhh.
bwwbwbbb
bbbbwwbb
bwwbwwwb
bbbbwwbb
bwwbwbbb
cccccccc
.cccccc.
"""
PRESENTATION = """
........
........
...###..
.#.#..#.
.#.#..#.
.#.#..#.
...###..
........
"""


def svg(art, palette):
    rows = [r for r in art.strip("\n").split("\n")]
    assert len(rows) == 8 and all(len(r) == 8 for r in rows), "sprites are 8 x 8"
    rects = []
    for y, row in enumerate(rows):
        x = 0
        while x < 8:
            ch = row[x]
            if ch == ".":
                x += 1
                continue
            end = x
            while end + 1 < 8 and row[end + 1] == ch:
                end += 1
            rects.append(f'<rect x="{x}" y="{y}" width="{end - x + 1}" height="1" fill="{palette[ch]}"/>')
            x = end + 1
    return '<svg viewBox="0 0 8 8" xmlns="http://www.w3.org/2000/svg" shape-rendering="crispEdges">' + "".join(rects) + "</svg>\n"


(WEB / "ico-library.svg").write_text(svg(LIBRARY, {"o": "#ee7a22", "d": "#b8520f", "h": "#f9a04a", "w": "#ffffff"}))
(WEB / "ico-playlist.svg").write_text(svg(PLAYLIST, {"b": "#3a85e6", "c": "#2560b8", "h": "#6aa6f2", "w": "#ffffff"}))
(WEB / "ico-presentation.svg").write_text(svg(PRESENTATION, {"#": "#000000"}))      # used as a mask: the page colours it

# --- title icons for the main panel: New presentation, Replace slides, Append slides, History ---
TITLE_ICONS = {
    "new": ("""
.hhhhhh.
gggwwggg
gggwwggg
gwwwwwwg
gggwwggg
gggwwggg
dddddddd
.dddddd.
""", {"g": "#3fae5a", "h": "#62cf7a", "d": "#26803f", "w": "#ffffff"}),
    "replace": ("""
.hhhhhh.
aaaaawaa
awwwwwwa
aaaaaaaa
awwwwwwa
aawaaaaa
dddddddd
.dddddd.
""", {"a": "#c4801a", "h": "#e8b040", "d": "#94600c", "w": "#ffffff"}),
    "append": ("""
.hhhhhh.
twwwwwwt
ttwwwwtt
tttwwttt
tttttttt
twwwwwwt
dddddddd
.dddddd.
""", {"t": "#26a69a", "h": "#4fd1c5", "d": "#17776e", "w": "#ffffff"}),
    "history": ("""
.hhhhhh.
ppwwwwpp
pwpwppwp
pwpwwpwp
pwppppwp
ppwwwwpp
dddddddd
.dddddd.
""", {"p": "#8b6bd9", "h": "#ad92f0", "d": "#5f45a8", "w": "#ffffff"}),
}
for name, (art, palette) in TITLE_ICONS.items():
    (WEB / f"ico-{name}.svg").write_text(svg(art, palette))
print("pixel icons written")
