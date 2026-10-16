"""Package the world for a GitHub release: one zip per 10,240-block tile (the
heightmap grid, ≤ 2 GiB each), a base zip (level.dat, data packs, icon,
attribution), the manifests and the download scripts.

    python tools/package_release.py --out D:\\release [--world C:\\mc1m_world\\japan_dem_1m]
                                    [--tiles all | "X,Z;X,Z"] [--jobs N] [--from-build [--threads N] [--tmp DIR]]
                                    [--manifest-only]

Reads release.json (repo, tag, prefix, world name). Writes to --out:
    <prefix>_base.zip, <prefix>_x<X>_z<Z>.zip ...  release assets
    tiles.tsv, tiles.json                          manifests (also docs/data/tiles.json for the pages)
    download.ps1, download.sh                      download scripts with the repo/tag filled in
    upload.ps1                                     `gh release` commands to publish everything

--from-build regenerates each tile's regions into a temporary world with
`jpdem world` instead of reading --world (use it when the world has been
played in and should not be published as is); the temporary worlds go to
<tmp or out>/_build and each jpdem uses --threads threads. --manifest-only
writes the manifests from the region files without zipping (sizes are estimates).
Finished tiles are logged in <out>/_done.jsonl: run again after an interruption
or a failed tile and only the missing zips are made.
"""
import argparse
import concurrent.futures as cf
import csv
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
import jpproj  # noqa: E402

TILE = 10240
# regions around the world spawn (summit of Mt. Fuji) ship with the base package,
# so a world with only some tiles still has ground under the player at first start
SPAWN_REGIONS = [(rx, rz) for rz in (136, 137, 138) for rx in (39, 40, 41)]
EXE = os.path.join(ROOT, "target", "release", "jpdem.exe" if os.name == "nt" else "jpdem")
HEIGHTMAP = r"E:\data\heightmap"


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for b in iter(lambda: f.read(1 << 22), b""):
            h.update(b)
    return h.hexdigest()


def tile_id(x, z):
    return f"x{x}_z{z}"


def tile_regions(x, z):
    # the spawn regions are only in the base package, so adding the Mt. Fuji tiles later
    # never replaces what a player built at the spawn
    rx0, rz0 = x // 512, z // 512
    spawn = set(SPAWN_REGIONS)
    return [(rx, rz) for rz in range(rz0, rz0 + TILE // 512) for rx in range(rx0, rx0 + TILE // 512) if (rx, rz) not in spawn]


def tile_info(heightmap):
    """Tiles of the heightmap grid with land share and elevation range."""
    out = []
    for r in csv.DictReader(open(os.path.join(heightmap, "tiles_detail.csv"), encoding="utf-8")):
        x, z = int(r["x"]), int(r["z"])
        land = int(r["px_land_1m"]) + int(r["px_land_5m"]) + int(r["px_land_filled"])
        inside = TILE * TILE - int(r["px_outside"])
        out.append({"x": x, "z": z, "land": round(land / (TILE * TILE), 4), "inside": round(inside / (TILE * TILE), 4),
                    "elev_min": float(r["min_elevation_m"]), "elev_max": float(r["max_elevation_m"])})
    return out


def zip_tile(args):
    x, z, region_dir, out_path, level = args
    files = []
    for rx, rz in tile_regions(x, z):
        p = os.path.join(region_dir, f"r.{rx}.{rz}.mca")
        # (an empty file is one the game created without saving anything)
        if os.path.exists(p) and os.path.getsize(p) > 0:
            files.append((p, f"region/r.{rx}.{rz}.mca"))
    if not files:
        return None
    tmp = out_path + ".tmp"
    with zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED, compresslevel=level) as zf:
        for p, arc in files:
            zf.write(p, arc)
    os.replace(tmp, out_path)
    return {"regions": len(files), "raw": sum(os.path.getsize(p) for p, _ in files), "size": os.path.getsize(out_path), "sha256": sha256(out_path)}


def build_and_zip(args):
    """--from-build: regenerate the tile's regions into a temporary world, zip, delete."""
    x, z, out_path, level, tmp_root, threads = args
    world = tempfile.mkdtemp(prefix=f"tile_{x}_{z}_", dir=tmp_root)
    try:
        regs = ";".join(f"{rx},{rz}" for rx, rz in tile_regions(x, z))
        env = dict(os.environ, RAYON_NUM_THREADS=str(threads))
        r = subprocess.run([EXE, "world", "--world", world, "--regions", regs, "--force"], env=env,
                           capture_output=True, text=True, encoding="utf-8", errors="replace")
        if r.returncode != 0:
            raise RuntimeError(f"jpdem world exited with {r.returncode}:\n{r.stderr[-2000:]}")
        return zip_tile((x, z, os.path.join(world, "region"), out_path, level))
    finally:
        shutil.rmtree(world, ignore_errors=True)


def fill(template, cfg):
    s = open(os.path.join(ROOT, "dist", template), encoding="utf-8-sig", newline="").read()
    for k in ("repo", "tag", "world", "prefix", "pages_url"):
        s = s.replace(f"__{k.upper()}__", cfg.get(k, ""))
    return s


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--world", default=r"C:\mc1m_world\japan_dem_1m")
    ap.add_argument("--heightmap", default=HEIGHTMAP)
    ap.add_argument("--tiles", default="all")
    ap.add_argument("--jobs", type=int, default=0, help="tiles at a time (default: CPUs / 2, with --from-build CPUs / 4: each jpdem needs about 6 GB)")
    ap.add_argument("--level", type=int, default=6)
    ap.add_argument("--from-build", action="store_true")
    ap.add_argument("--threads", type=int, default=0, help="threads per jpdem with --from-build (default: 2 x CPUs / jobs)")
    ap.add_argument("--tmp", help="folder for the temporary --from-build worlds (default: --out)")
    ap.add_argument("--manifest-only", action="store_true")
    a = ap.parse_args()
    if os.name == "nt":  # no idle sleep during a long run (ES_CONTINUOUS | ES_SYSTEM_REQUIRED, ends with the process)
        import ctypes
        ctypes.windll.kernel32.SetThreadExecutionState(0x80000001)
    cpus = os.cpu_count() or 4
    # a jpdem tile build is ~7 s of single-threaded setup, the parallel part and a single-threaded zip,
    # so several tiles at once with a few threads each keep all cores busy
    a.jobs = a.jobs or max(1, cpus // (4 if a.from_build else 2))
    cfg = json.load(open(os.path.join(ROOT, "release.json"), encoding="utf-8"))
    os.makedirs(a.out, exist_ok=True)
    region_dir = os.path.join(a.world, "region")
    tiles = tile_info(a.heightmap)
    if a.tiles != "all":
        want = set()
        for p in a.tiles.split(";"):
            x, z = (int(v) for v in p.split(","))
            want.add((x // TILE * TILE, z // TILE * TILE))
        tiles = [t for t in tiles if (t["x"], t["z"]) in want]
    old = {}
    mpath = os.path.join(a.out, "tiles.json")
    if os.path.exists(mpath):
        old = {t["id"]: t for t in json.load(open(mpath, encoding="utf-8")).get("tiles", [])}
    done_log = os.path.join(a.out, "_done.jsonl")
    if os.path.exists(done_log) and not a.manifest_only:
        for line in open(done_log, encoding="utf-8"):
            try:
                d = json.loads(line)
            except ValueError:
                continue  # a line cut short by an interrupted run
            old[d["id"]] = d

    # ---- base package
    base_zip = os.path.join(a.out, f"{cfg['prefix']}_base.zip")
    base = None
    if not a.manifest_only:
        stage = tempfile.mkdtemp(prefix="base_")
        try:
            wdir = os.path.join(stage, cfg["world"])
            subprocess.run([EXE, "base", "--world", wdir, "--heightmap", a.heightmap], check=True)
            os.makedirs(os.path.join(wdir, "region"), exist_ok=True)
            # always freshly generated (a few seconds): never the spawn area of a world someone played in
            regs = ";".join(f"{rx},{rz}" for rx, rz in SPAWN_REGIONS)
            subprocess.run([EXE, "world", "--world", wdir, "--regions", regs, "--force"], check=True, capture_output=True)
            if os.path.exists(os.path.join(ROOT, "dist", "WORLD_README.txt")):
                with open(os.path.join(wdir, "README.txt"), "w", encoding="utf-8", newline="") as f:
                    f.write(fill("WORLD_README.txt", cfg))
            with zipfile.ZipFile(base_zip, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
                for d, _, fs in os.walk(wdir):
                    for f in fs:
                        p = os.path.join(d, f)
                        zf.write(p, os.path.relpath(p, wdir).replace(os.sep, "/"))
        finally:
            shutil.rmtree(stage, ignore_errors=True)
        base = {"file": os.path.basename(base_zip), "size": os.path.getsize(base_zip), "sha256": sha256(base_zip)}
    elif os.path.exists(mpath):
        base = json.load(open(mpath, encoding="utf-8")).get("base")

    # ---- tiles
    t0 = time.time()
    entries, jobs = [], []
    for t in tiles:
        tid = tile_id(t["x"], t["z"])
        name = f"{cfg['prefix']}_{tid}.zip"
        out_path = os.path.join(a.out, name)
        lat, lon = jpproj.mc_to_ll(t["x"] + TILE / 2, t["z"] + TILE / 2)
        e = {"id": tid, "x": t["x"], "z": t["z"], "file": name, "center": [round(lat, 4), round(lon, 4)],
             "land": t["land"], "inside": t["inside"], "elev_max": t["elev_max"]}
        entries.append(e)
        if a.manifest_only:
            files = [os.path.join(region_dir, f"r.{rx}.{rz}.mca") for rx, rz in tile_regions(t["x"], t["z"])]
            files = [f for f in files if os.path.exists(f)]
            raw = sum(os.path.getsize(f) for f in files)
            e.update({"regions": len(files), "raw": raw, "size": int(raw * (0.15 + 0.62 * min(1.0, t["land"] / max(t["inside"], 1e-9)))), "estimated": True})
            continue
        if os.path.exists(out_path) and tid in old and old[tid].get("size") == os.path.getsize(out_path) and not old[tid].get("estimated"):
            e.update({k: old[tid][k] for k in ("regions", "raw", "size", "sha256")})
            continue
        jobs.append((e, out_path))
    # land tiles take longest: start them first so the run does not end on one big tile
    jobs.sort(key=lambda j: -j[0]["land"])
    done, failed = 0, []
    if jobs:
        print(f"{len(jobs)} tiles to zip{' (building them first)' if a.from_build else ''}, {len(entries) - len(jobs)} kept", flush=True)
        tmp_root = os.path.join(a.tmp or a.out, "_build")
        with cf.ProcessPoolExecutor(a.jobs) as ex:
            if a.from_build:
                shutil.rmtree(tmp_root, ignore_errors=True)  # leftovers of an interrupted run
                os.makedirs(tmp_root)
                threads = a.threads or max(1, cpus * 2 // a.jobs)
                print(f"building with {a.jobs} jpdem processes x {threads} threads in {tmp_root}", flush=True)
                futs = {ex.submit(build_and_zip, (e["x"], e["z"], p, a.level, tmp_root, threads)): e for e, p in jobs}
            else:
                futs = {ex.submit(zip_tile, (e["x"], e["z"], region_dir, p, a.level)): e for e, p in jobs}
            for f in cf.as_completed(futs):
                e = futs[f]
                try:
                    r = f.result()
                except Exception as err:  # keep going: the finished tiles are logged and a re-run retries the rest
                    failed.append(e["id"])
                    print(f"  FAILED {e['id']}: {err}", flush=True)
                    r = None
                if r:
                    e.update(r)
                    with open(done_log, "a", encoding="utf-8") as lf:
                        lf.write(json.dumps({"id": e["id"], **r}) + "\n")
                done += 1
                if done % 20 == 0 or done == len(jobs):
                    el = time.time() - t0
                    print(f"  {done}/{len(jobs)} tiles zipped, {el:.0f}s elapsed, ~{el / done * (len(jobs) - done):.0f}s left", flush=True)
        if a.from_build:
            shutil.rmtree(tmp_root, ignore_errors=True)
    if failed:
        sys.exit(f"{len(failed)} tiles failed ({' '.join(failed)}): run the same command again to retry them")
    entries = [e for e in entries if e.get("regions")]
    entries.sort(key=lambda e: (e["z"], e["x"]))

    # ---- manifests
    manifest = {
        "repo": cfg["repo"], "tag": cfg["tag"], "world": cfg["world"], "minecraft": cfg.get("minecraft", ""),
        "base_url": f"https://github.com/{cfg['repo']}/releases/download/{cfg['tag']}/",
        "tile_size": TILE, "base": base, "tiles": entries,
    }
    json.dump(manifest, open(mpath, "w", encoding="utf-8"), ensure_ascii=False, indent=0)
    if a.tiles == "all":  # a test run with a few tiles must not shrink the pages' tile map
        os.makedirs(os.path.join(ROOT, "docs", "data"), exist_ok=True)
        json.dump(manifest, open(os.path.join(ROOT, "docs", "data", "tiles.json"), "w", encoding="utf-8"), ensure_ascii=False, separators=(",", ":"))
    with open(os.path.join(a.out, "tiles.tsv"), "w", encoding="utf-8", newline="\n") as f:
        f.write("id\tfile\tsize\tsha256\n")
        if base:
            f.write(f"base\t{base['file']}\t{base['size']}\t{base['sha256']}\n")
        for e in entries:
            f.write(f"{e['id']}\t{e['file']}\t{e['size']}\t{e.get('sha256', '')}\n")
    for script in ("download.ps1", "download.sh"):
        ps = script.endswith(".ps1")
        # Windows PowerShell 5.1 reads non-ASCII scripts as UTF-8 only with a BOM
        with open(os.path.join(a.out, script), "w", encoding="utf-8-sig" if ps else "utf-8", newline="\r\n" if ps else "\n") as f:
            f.write(fill(script, cfg).replace("\r\n", "\n"))
    assets = ([base["file"]] if base else []) + [e["file"] for e in entries] + ["tiles.tsv", "tiles.json", "download.ps1", "download.sh"]
    with open(os.path.join(a.out, "upload.ps1"), "w", encoding="utf-8", newline="\r\n") as f:
        f.write(f"# Publish the release with the GitHub CLI (gh auth login first). Run in {a.out}\n")
        f.write(f"gh release create {cfg['tag']} --repo {cfg['repo']} --title \"{cfg['tag']}\" --notes \"Japan 1:1 Minecraft world ({cfg.get('minecraft', '')})\"\n")
        for i in range(0, len(assets), 20):
            f.write(f"gh release upload {cfg['tag']} --repo {cfg['repo']} --clobber " + " ".join(assets[i:i + 20]) + "\n")
    total = sum(e["size"] for e in entries) + (base["size"] if base else 0)
    print(f"{len(entries)} tiles, {total / 1e9:.1f} GB{' (estimated)' if a.manifest_only else ''} -> {a.out}")


if __name__ == "__main__":
    main()
