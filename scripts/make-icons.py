"""Draws the oxidows app icons into crates/ui-common/assets/ (.ico for the exe, .png for the window).

Every app gets the same tile: a rounded square with a diagonal gradient in the app's color and a
white glyph drawn with the same stroke weight. Run from the repo root: python scripts/make-icons.py
(needs Pillow). The output is committed, so builds don't need Python.
"""

from pathlib import Path

from PIL import Image, ImageChops, ImageDraw

OUT = Path(__file__).resolve().parent.parent / "crates" / "ui-common" / "assets"
SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]
SS = 8  # supersampling factor

APPS = {
    # name: (gradient start, gradient end, glyph)
    "taskmgr": ((0x0A, 0x5F, 0xC2), (0x1F, 0xB8, 0xD8), "pulse"),
    "explorer": ((0xE8, 0x96, 0x00), (0xFF, 0xCB, 0x3D), "folder"),
}


def gradient(n, a, b):
    """Diagonal gradient from the top-left corner (a) to the bottom-right (b)."""
    gx = Image.linear_gradient("L").rotate(90).resize((n, n))  # 0 at the left, 255 at the right
    gy = Image.linear_gradient("L").resize((n, n))  # 0 at the top, 255 at the bottom
    g = ImageChops.add(gx, gy, scale=2.0)
    ca = Image.new("RGB", (n, n), a)
    cb = Image.new("RGB", (n, n), b)
    return Image.composite(cb, ca, g)


def tile_mask(n, inset, radius):
    m = Image.new("L", (n, n), 0)
    ImageDraw.Draw(m).rounded_rectangle([inset, inset, n - 1 - inset, n - 1 - inset], radius, fill=255)
    return m


def glyph(kind, n, weight):
    """White glyph mask on an n x n canvas; `weight` is the stroke width in canvas pixels."""
    m = Image.new("L", (n, n), 0)
    d = ImageDraw.Draw(m)
    u = n / 100.0
    if kind == "pulse":
        pts = [(20, 54), (36, 54), (44, 34), (56, 72), (64, 48), (70, 54), (80, 54)]
        pts = [(x * u, y * u) for x, y in pts]
        d.line(pts, fill=255, width=int(weight), joint="curve")
        r = weight / 2
        for x, y in (pts[0], pts[-1]):
            d.ellipse([x - r, y - r, x + r, y + r], fill=255)
    elif kind == "folder":
        # Back panel with its tab, then the front panel slightly lower and lighter cut.
        back = Image.new("L", (n, n), 0)
        bd = ImageDraw.Draw(back)
        bd.rounded_rectangle([22 * u, 28 * u, 78 * u, 74 * u], 6 * u, fill=190)
        bd.rounded_rectangle([22 * u, 24 * u, 46 * u, 36 * u], 5 * u, fill=190)
        front = Image.new("L", (n, n), 0)
        ImageDraw.Draw(front).rounded_rectangle([22 * u, 38 * u, 78 * u, 74 * u], 6 * u, fill=255)
        m = ImageChops.lighter(back, front)
    return m


def render(app, size):
    a, b, kind = APPS[app]
    n = size * SS
    # Small sizes: thicker strokes and a smaller margin so the glyph stays legible.
    inset = n * (0.04 if size <= 24 else 0.06)
    radius = n * 0.22
    weight = n * (0.10 if size <= 24 else 0.075)
    img = gradient(n, a, b).convert("RGBA")
    white = Image.new("RGBA", (n, n), (255, 255, 255, 255))
    img = Image.composite(white, img, glyph(kind, n, weight))
    img.putalpha(tile_mask(n, inset, radius))
    return img.resize((size, size), Image.Resampling.LANCZOS)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    for app in APPS:
        imgs = {s: render(app, s) for s in SIZES}
        imgs[256].save(OUT / f"{app}.png", optimize=True)
        imgs[256].save(
            OUT / f"{app}.ico",
            sizes=[(s, s) for s in SIZES],
            append_images=[imgs[s] for s in SIZES if s != 256],
        )
        print(OUT / f"{app}.ico")


if __name__ == "__main__":
    main()
