"""ProPresenter's own sidebar icons (library, playlist, presentation), read from the installed ProPresenter app.

They are shown as 8-bit sprites (12 x 12 pixels, 3-3-2 bit colour). They are ProPresenter's artwork, so VerseVision does not ship them: ``versevision icons`` renders them out of the app on this Mac into the
user data folder, and the web page uses them when they are there (and a plain look when they are not)."""
from __future__ import annotations

import glob
import subprocess
import sys
import tempfile
from pathlib import Path

from platformdirs import user_data_path

from .errors import VerseVisionError

NAMES = {"library": "OutlineSourceLibraryItem", "playlist": "OutlineSourcePresentationItem", "presentation": "OutlineDetailPresentationItem"}
GRID = 12        # the sprite is 12 x 12 pixels...
BLOCK = 4        # ...each stored as a 4 x 4 block, so the file is 48 px and stays hard-edged at any scale


def icon_dir() -> Path:
    return user_data_path("versevision") / "propresenter-icons"


def icon_bytes(kind: str) -> bytes | None:
    """The PNG for ``kind`` if it was extracted, else None."""
    if kind not in NAMES:
        return None
    path = icon_dir() / f"{kind}.png"
    return path.read_bytes() if path.is_file() else None


_SWIFT = """
import AppKit
let args = CommandLine.arguments
let bundle = Bundle(path: args[1])!
let out = args[2]
let names = ["library": "%(library)s", "playlist": "%(playlist)s", "presentation": "%(presentation)s"]
let big = 192, grid = %(grid)d, block = %(block)d       // draw big, shrink to a grid x grid sprite, then blow each pixel up into a block x block square
func render(_ img: NSImage, _ px: Int) -> NSBitmapImageRep {
  let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
  NSGraphicsContext.saveGraphicsState()
  NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
  img.draw(in: NSRect(x: 0, y: 0, width: px, height: px), from: .zero, operation: .sourceOver, fraction: 1)
  NSGraphicsContext.restoreGraphicsState()
  return rep
}
func level(_ v: Double, _ steps: Double) -> UInt8 { UInt8((v / 255.0 * steps).rounded() / steps * 255.0) }     // 8-bit colour: 3 bits red, 3 green, 2 blue
for (kind, name) in names {
  guard let img = bundle.image(forResource: name) else { print("missing", name); exit(2) }
  let src = render(img, big)
  let cell = big / grid
  let sprite = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: grid * block, pixelsHigh: grid * block, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
  for gy in 0..<grid { for gx in 0..<grid {
    var r = 0.0, g = 0.0, b = 0.0, a = 0.0
    for y in 0..<cell { for x in 0..<cell {
      let c = src.colorAt(x: gx * cell + x, y: gy * cell + y)!.usingColorSpace(.deviceRGB)!
      let al = Double(c.alphaComponent); a += al; r += Double(c.redComponent) * al; g += Double(c.greenComponent) * al; b += Double(c.blueComponent) * al
    } }
    let n = Double(cell * cell)
    let color: NSColor
    if a / n < 0.45 { color = NSColor(deviceRed: 0, green: 0, blue: 0, alpha: 0) }
    else { color = NSColor(deviceRed: CGFloat(level(r / a * 255, 7)) / 255, green: CGFloat(level(g / a * 255, 7)) / 255, blue: CGFloat(level(b / a * 255, 3)) / 255, alpha: 1) }
    for y in 0..<block { for x in 0..<block { sprite.setColor(color, atX: gx * block + x, y: gy * block + y) } }
  } }
  try! sprite.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out + "/" + kind + ".png"))
}
""" % {**NAMES, "grid": GRID, "block": BLOCK}


def extract(app: str | None = None) -> Path:
    """Render the three icons out of ProPresenter's app bundle. macOS only; needs the Xcode command line tools (swiftc)."""
    if sys.platform != "darwin":
        raise VerseVisionError("ProPresenter's icons can only be read on the Mac that has ProPresenter installed")
    found = app or next(iter(sorted(glob.glob("/Applications/ProPresenter*.app"), reverse=True)), None)
    if not found:
        raise VerseVisionError("ProPresenter is not installed in /Applications")
    target = icon_dir()
    target.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="versevision-icons-") as tmp:
        source, binary = Path(tmp) / "icons.swift", Path(tmp) / "icons"
        source.write_text(_SWIFT)
        build = subprocess.run(["swiftc", "-O", str(source), "-o", str(binary)], capture_output=True, text=True, timeout=300)
        if build.returncode != 0:
            raise VerseVisionError("Could not build the icon reader (is the Xcode command line tools package installed?): " + build.stderr.strip()[-300:])
        run = subprocess.run([str(binary), found, str(target)], capture_output=True, text=True, timeout=60)
        if run.returncode != 0:
            raise VerseVisionError("Could not read the icons from ProPresenter: " + (run.stdout + run.stderr).strip()[-300:])
    return target
