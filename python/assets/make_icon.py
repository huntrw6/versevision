"""Render assets/versevision-icon.svg's artwork to PNG and .icns with Pillow (no SVG renderer needed). Run: python3 assets/make_icon.py"""
import math
from pathlib import Path
from PIL import Image, ImageDraw

HERE = Path(__file__).parent
S, N = 2048, 512                      # draw at 4x the 512 design grid
k = S / N


def lerp(a, b, t):
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))


def vgrad(w, h, top, bottom):
    img = Image.new("RGB", (w, h))
    px = img.load()
    for y in range(h):
        c = lerp(top, bottom, y / max(h - 1, 1))
        for x in range(w):
            px[x, y] = c
    return img


canvas = Image.new("RGBA", (S, S), (0, 0, 0, 0))
mask = Image.new("L", (S, S), 0)
ImageDraw.Draw(mask).rounded_rectangle((0, 0, S - 1, S - 1), radius=int(115 * k), fill=255)
canvas.paste(vgrad(S, S, (0x2b, 0x2b, 0x30), (0x12, 0x12, 0x14)).convert("RGBA"), (0, 0), mask)

page = [(96, 170), (256, 400), (416, 170), (416, 330), (256, 440), (96, 330)]
pm = Image.new("L", (S, S), 0)
ImageDraw.Draw(pm).polygon([(x * k, y * k) for x, y in page], fill=255)
grad_page = Image.new("RGBA", (S, S), (0, 0, 0, 0))
grad_page.paste(vgrad(S, int(270 * k), (0xf4, 0xf1, 0xea), (0xd9, 0xd4, 0xc8)).convert("RGBA"), (0, int(170 * k)))
canvas.paste(grad_page, (0, 0), pm)

vm = Image.new("L", (S, S), 0)
d = ImageDraw.Draw(vm)
pts = [(150, 170), (256, 330), (362, 170)]
r = 44 * k / 2
for (x1, y1), (x2, y2) in zip(pts, pts[1:]):
    dx, dy = x2 - x1, y2 - y1
    length = math.hypot(dx, dy)
    nx, ny = -dy / length * r, dx / length * r
    d.polygon([(x1 * k + nx, y1 * k + ny), (x2 * k + nx, y2 * k + ny), (x2 * k - nx, y2 * k - ny), (x1 * k - nx, y1 * k - ny)], fill=255)
for x, y in pts:
    d.ellipse((x * k - r, y * k - r, x * k + r, y * k + r), fill=255)
orange = Image.new("RGBA", (S, S), (0, 0, 0, 0))
orange.paste(vgrad(S, int((330 - 170 + 44) * k), (0xff, 0xa4, 0x3a), (0xf2, 0x6a, 0x10)).convert("RGBA"), (0, int((170 - 22) * k)))
canvas.paste(orange, (0, 0), vm)

master = canvas.resize((1024, 1024), Image.LANCZOS)
master.save(HERE / "versevision-icon-1024.png")
master.save(HERE / "versevision.icns", sizes=[(16, 16), (32, 32), (64, 64), (128, 128), (256, 256), (512, 512), (1024, 1024)])
master.resize((64, 64), Image.LANCZOS).save(HERE / "versevision-icon-64.png")
print("done")


# --- web-app (PWA) icons, shipped inside the package so the page can serve them ---
WEB = HERE.parent / "src" / "versevision" / "web"
for size in (180, 192, 512):
    master.resize((size, size), Image.LANCZOS).save(WEB / f"icon-{size}.png")
# maskable: the system crops it to its own shape, so the artwork sits inside the middle 80% on a full-bleed background
full = Image.new("RGBA", (S, S), (0, 0, 0, 0))
full.paste(vgrad(S, S, (0x2b, 0x2b, 0x30), (0x12, 0x12, 0x14)).convert("RGBA"), (0, 0))
art_layer = Image.new("RGBA", (S, S), (0, 0, 0, 0))                                     # the page + V alone, no tile behind them
art_layer.paste(grad_page, (0, 0), pm)
art_layer.paste(orange, (0, 0), vm)
art = art_layer.crop(art_layer.getbbox())
scale = 0.62 * S / art.width
art = art.resize((int(art.width * scale), int(art.height * scale)), Image.LANCZOS)
full.alpha_composite(art, ((S - art.width) // 2, (S - art.height) // 2))
full.resize((512, 512), Image.LANCZOS).save(WEB / "icon-maskable-512.png")
print("web icons done")
