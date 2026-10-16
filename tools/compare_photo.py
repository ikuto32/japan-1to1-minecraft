"""Side-by-side check image: a top-down render of the world (left) next to the
GSI aerial photo (地理院タイル「全国最新写真（シームレス）」, right) resampled to
the same Minecraft grid (1 px = 1 block), so positions can be compared directly.

usage: python compare_photo.py <render.png> <render_x0> <render_z0> <x0> <z0> <w> <h> <out.png> [zoom]
  render.png      top-down render (jpdem verify render ...) whose pixel (0,0) is MC (render_x0, render_z0)
  x0 z0 w h       MC window to compare
Photo tiles are cached under %TEMP%/gsi_photo. Source: 国土地理院 地理院タイル (attribution in the image).
"""
import io
import math
import os
import sys
import urllib.request

from PIL import Image, ImageDraw, ImageFont

A = 6378137.0
F = 1 / 298.257222101
E2 = F * (2 - F)
E = math.sqrt(E2)
LAT1, LAT2, LAT0, LON0 = map(math.radians, (30.0, 40.0, 36.0, 138.5))


def _m(p):
    return math.cos(p) / math.sqrt(1 - E2 * math.sin(p) ** 2)


def _t(p):
    s = math.sin(p)
    return math.tan(math.pi / 4 - p / 2) / ((1 - E * s) / (1 + E * s)) ** (E / 2)


N = (math.log(_m(LAT1)) - math.log(_m(LAT2))) / (math.log(_t(LAT1)) - math.log(_t(LAT2)))
AF = A * _m(LAT1) / (N * _t(LAT1) ** N)
RHO0 = AF * _t(LAT0) ** N


def mc_to_ll(x, z):
    """Minecraft (X, Z) -> (lat, lon) degrees (LCC 2SP, origin 36N 138.5E, Z = -northing)."""
    y = -z
    dy = RHO0 - y
    rho = math.copysign(math.hypot(x, dy), N)
    th = math.atan2(x, dy)
    t = (rho / AF) ** (1 / N)
    p = math.pi / 2 - 2 * math.atan(t)
    for _ in range(15):
        s = math.sin(p)
        p = math.pi / 2 - 2 * math.atan(t * ((1 - E * s) / (1 + E * s)) ** (E / 2))
    return math.degrees(p), math.degrees(th / N + LON0)


def merc_px(lat, lon, z):
    n = 256 * 2 ** z
    x = (lon + 180) / 360 * n
    s = math.sin(math.radians(lat))
    y = (0.5 - math.log((1 + s) / (1 - s)) / (4 * math.pi)) * n
    return x, y


cache_dir = os.path.join(os.environ.get("TEMP", "."), "gsi_photo")
os.makedirs(cache_dir, exist_ok=True)
_tiles = {}


def tile(z, tx, ty):
    k = (z, tx, ty)
    if k in _tiles:
        return _tiles[k]
    p = os.path.join(cache_dir, f"{z}_{tx}_{ty}.jpg")
    if not os.path.exists(p):
        url = f"https://cyberjapandata.gsi.go.jp/xyz/seamlessphoto/{z}/{tx}/{ty}.jpg"
        req = urllib.request.Request(url, headers={"User-Agent": "jpdem-verify/1.0"})
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                open(p, "wb").write(r.read())
        except Exception:
            _tiles[k] = None
            return None
    _tiles[k] = Image.open(p).convert("RGB")
    return _tiles[k]


def main():
    render, rx0, rz0, x0, z0, w, h, out = sys.argv[1], *map(int, sys.argv[2:8]), sys.argv[8]
    zoom = int(sys.argv[9]) if len(sys.argv) > 9 else 17
    ren = Image.open(render).convert("RGB").crop((x0 - rx0, z0 - rz0, x0 - rx0 + w, z0 - rz0 + h))
    photo = Image.new("RGB", (w, h))
    px = photo.load()
    # exact projection on a coarse lattice, bilinear in between
    step = 16
    lat_grid = {}
    for j in range(0, h + step, step):
        for i in range(0, w + step, step):
            lat_grid[(i, j)] = merc_px(*mc_to_ll(x0 + i + 0.5, z0 + j + 0.5), zoom)
    for j in range(h):
        j0 = (j // step) * step
        fj = (j - j0) / step
        for i in range(w):
            i0 = (i // step) * step
            fi = (i - i0) / step
            a, b, c, d = lat_grid[(i0, j0)], lat_grid[(i0 + step, j0)], lat_grid[(i0, j0 + step)], lat_grid[(i0 + step, j0 + step)]
            mx = (a[0] * (1 - fi) + b[0] * fi) * (1 - fj) + (c[0] * (1 - fi) + d[0] * fi) * fj
            my = (a[1] * (1 - fi) + b[1] * fi) * (1 - fj) + (c[1] * (1 - fi) + d[1] * fi) * fj
            t = tile(zoom, int(mx // 256), int(my // 256))
            if t is not None:
                px[i, j] = t.getpixel((int(mx % 256), int(my % 256)))
    canvas = Image.new("RGB", (w * 2 + 8, h + 22), (255, 255, 255))
    canvas.paste(ren, (0, 0))
    canvas.paste(photo, (w + 8, 0))
    d = ImageDraw.Draw(canvas)
    font = None
    for f in ("C:/Windows/Fonts/meiryo.ttc", "C:/Windows/Fonts/msgothic.ttc", "C:/Windows/Fonts/YuGothM.ttc"):
        if os.path.exists(f):
            font = ImageFont.truetype(f, 13)
            break
    d.text((4, h + 3), f"Minecraft X {x0}..{x0 + w}  Z {z0}..{z0 + h}（1 px = 1 ブロック）", fill=(0, 0, 0), font=font)
    d.text((w + 12, h + 3), "出典: 国土地理院 地理院タイル（全国最新写真・シームレス）", fill=(0, 0, 0), font=font)
    canvas.save(out)


if __name__ == "__main__":
    main()
