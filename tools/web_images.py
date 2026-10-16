"""Small web versions of the check images for the gallery in docs/index.html:
docs/check/<name>.png -> docs/check/web/<name>.webp (the gallery still links
each one to its full-size PNG).

usage: uv run --no-project --with pillow python -I tools/web_images.py [name ...]

The gallery crops every image to a 2:1 box (object-fit: cover), so each one is
scaled until that centre window is 720x360 px, about twice the size of a
gallery cell on a desktop screen; a 2008x722 image becomes 1001x360.
"""
import glob
import os
import sys

from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "docs", "check")
OUT = os.path.join(SRC, "web")
BOX_W, BOX_H = 720, 360
QUALITY = 80


def main():
    names = sys.argv[1:] or sorted(os.path.splitext(os.path.basename(p))[0] for p in glob.glob(os.path.join(SRC, "*.png")))
    os.makedirs(OUT, exist_ok=True)
    for name in names:
        im = Image.open(os.path.join(SRC, f"{name}.png")).convert("RGB")
        w, h = im.size
        s = min(1.0, max(BOX_W / w, BOX_H / h))
        im = im.resize((round(w * s), round(h * s)), Image.Resampling.LANCZOS)
        out = os.path.join(OUT, f"{name}.webp")
        im.save(out, "WEBP", quality=QUALITY, method=6)
        print(f"{name}: {w}x{h} -> {im.width}x{im.height}, {os.path.getsize(out) // 1024} KB", flush=True)


if __name__ == "__main__":
    main()
