"""Top-down check images of key spots: render the world around each spot
(jpdem verify render) and put it next to the GSI aerial photo (compare_photo.py).

usage: python spot_images.py <region_dir> <out_dir> [name ...]
"""
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
EXE = os.path.join(HERE, "..", "target", "release", "jpdem.exe")

# name: (center X, center Z, width, height) in Minecraft blocks
SPOTS = {
    "01_shizuoka_station": (-10094, 113663, 1000, 700),
    "02_shin_tomei_shin_shizuoka_ic": (-11100, 106300, 1200, 800),
    "03_tomei_shizuoka_ic": (-9600, 116250, 1200, 800),
    "04_fujikawa_shinkansen_bridge": (11880, 95260, 1700, 700),
    "04b_fujikawa_tomei_tokaido_bridges": (11300, 93300, 1700, 1600),
    "05_fuji_fujinomiya_trail": (20900, 72000, 1400, 3200),
    "06_makinohara_tea": (-31453, 133136, 1000, 700),
    "07_shizuoka_airport": (-28300, 133000, 1600, 1100),
    "08_aokigahara": (10851, 58581, 1000, 700),
    "09_kofu_station": (6224, 36810, 1000, 700),
    "10_tokyo_yurakucho_shimbashi": (113550, 35850, 1000, 1000),
    "11_tokyo_hamazakibashi_jct": (113700, 37700, 1200, 800),
}


def main():
    reg, out = sys.argv[1], sys.argv[2]
    names = sys.argv[3:] or list(SPOTS)
    os.makedirs(out, exist_ok=True)
    for name in names:
        cx, cz, w, h = SPOTS[name]
        x0, z0 = cx - w // 2, cz - h // 2
        rx0, rz0 = x0 // 512, z0 // 512
        rx1, rz1 = (x0 + w - 1) // 512, (z0 + h - 1) // 512
        ren = os.path.join(out, f"_{name}_render.png")
        subprocess.run([EXE, "verify", "render", reg, str(rx0), str(rz0), str(rx1 - rx0 + 1), str(rz1 - rz0 + 1), ren, "1"], check=True)
        subprocess.run(
            [sys.executable, os.path.join(HERE, "compare_photo.py"), ren, str(rx0 * 512), str(rz0 * 512), str(x0), str(z0), str(w), str(h), os.path.join(out, f"{name}.png")],
            check=True,
        )
        os.remove(ren)
        print(name, "ok", flush=True)


if __name__ == "__main__":
    main()
