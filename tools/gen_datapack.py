"""Generate the in-game tools data pack `japan_tools` (namespace `jp`) and the
landmark list for the web pages.

    python tools/gen_datapack.py [--probe]

Inputs : assets/places.tsv (landmarks), release.json (web page URL, optional)
Outputs: assets/datapacks/japan_tools/   (copied into the world by `jpdem world`)
         assets/places.json               (landmarks with Minecraft X/Y/Z; --probe re-reads
                                           the ground height from the heightmap with jpdem hmprobe)
         docs/data/places.json            (same, for the web pages)

The on-screen latitude/longitude is a degree-2 polynomial of X/Z evaluated in
32-bit scoreboard arithmetic; the program below is simulated with the same
integer semantics and checked against the exact projection before writing.
"""
import csv
import json
import math
import os
import re
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
import jpproj  # noqa: E402

PACK = os.path.join(ROOT, "assets", "datapacks", "japan_tools")
FN = os.path.join(PACK, "data", "jp", "function")
TOP_Y = 2031  # world top (tall_japan)
SPAWN = (20583, 2016, 70641)  # summit of Mt. Fuji, the world spawn (its regions ship with the base package)
INT_MIN, INT_MAX = -(2**31), 2**31 - 1

GROUPS = [
    ("fuji", "富士山・富士五湖", "Mt. Fuji & Fuji Five Lakes"),
    ("shizuoka", "静岡", "Shizuoka"),
    ("izu", "伊豆・箱根", "Izu & Hakone"),
    ("kanto", "神奈川・東京西部", "Kanagawa & western Tokyo"),
    ("tokyo", "東京都心", "Central Tokyo"),
    ("yamanashi", "山梨", "Yamanashi"),
]


# ---------------------------------------------------------------- landmarks

def load_places(probe):
    cache = os.path.join(ROOT, "assets", "places.json")
    old = {}
    if os.path.exists(cache):
        old = {p["id"]: p for p in json.load(open(cache, encoding="utf-8"))}
    exe = os.path.join(ROOT, "target", "release", "jpdem.exe")
    places = []
    for r in csv.DictReader(open(os.path.join(ROOT, "assets", "places.tsv"), encoding="utf-8"), delimiter="\t"):
        lat, lon = float(r["lat"]), float(r["lon"])
        x, z = jpproj.ll_to_mc(lat, lon)
        x, z = round(x), round(z)
        y = old.get(r["id"], {}).get("y")
        if probe or y is None or old[r["id"]].get("x") != x or old[r["id"]].get("z") != z:
            out = subprocess.run([exe, "hmprobe", str(x), str(z)], capture_output=True, text=True, encoding="utf-8").stdout
            m = re.search(r"Minecraft surface Y = (-?\d+)", out)
            if not m:
                raise SystemExit(f"hmprobe failed for {r['id']}: {out[-300:]}")
            y = int(m.group(1))
        places.append({"id": r["id"], "ja": r["ja"], "en": r["en"], "group": r["group"], "lat": lat, "lon": lon, "x": x, "y": y, "z": z})
    json.dump(places, open(cache, "w", encoding="utf-8"), ensure_ascii=False, indent=1)
    os.makedirs(os.path.join(ROOT, "docs", "data"), exist_ok=True)
    json.dump(places, open(os.path.join(ROOT, "docs", "data", "places.json"), "w", encoding="utf-8"), ensure_ascii=False, indent=1)
    return places


# ---------------------------------------------------------------- lat/lon program

def fit(k):
    """Least-squares degree-2 polynomial of lat (k=0) or lon (k=1) in (X, Z) metres."""
    import numpy as np

    xs = np.linspace(-160000, 160000, 65)
    zs = np.linspace(-40000, 240000, 57)
    X, Z = np.meshgrid(xs, zs)
    X, Z = X.ravel(), Z.ravel()
    v = np.array([jpproj.mc_to_ll(x, z)[k] for x, z in zip(X, Z)]) * 1e6  # µdeg
    cols = [np.ones_like(X), X, Z, X * X, X * Z, Z * Z]
    c, *_ = np.linalg.lstsq(np.stack(cols, 1), v, rcond=None)
    return dict(zip(["1", "x", "z", "xx", "xz", "zz"], c))


def ratio(c, maxk):
    """Integer K/D ≈ c with |K| ≤ maxk and D a power of ten, as precise as possible."""
    d = 1
    while abs(c * d * 10) <= maxk and d < 10**9:
        d *= 10
    return round(c * d), d


class Prog:
    """Tiny scoreboard program: emits commands and simulates them (Java int, floorDiv)."""

    def __init__(self):
        self.ops = []
        self.consts = set()

    def const(self, v):
        self.consts.add(v)
        return f"#k{v}".replace("-", "m")

    def op(self, dst, o, src):
        self.ops.append((dst, o, src))

    def set(self, dst, v):
        self.ops.append((dst, "set", v))

    def emit(self, obj="jp_t", cobj="jp_c"):
        out = []
        for dst, o, src in self.ops:
            if o == "set":
                out.append(f"scoreboard players set {dst} {obj} {src}")
            elif isinstance(src, int):
                out.append(f"scoreboard players operation {dst} {obj} {o} {self.const(src)} {cobj}")
            else:
                out.append(f"scoreboard players operation {dst} {obj} {o} {src} {obj}")
        return out

    def run(self, env):
        env = dict(env)
        for dst, o, src in self.ops:
            if o == "set":
                env[dst] = src
                continue
            b = src if isinstance(src, int) else env[src]
            a = env.get(dst, 0)
            if o == "=":
                r = b
            elif o == "+=":
                r = a + b
            elif o == "-=":
                r = a - b
            elif o == "*=":
                r = a * b
            elif o == "/=":
                r = a // b
            elif o == "%=":
                r = a % b
            else:
                raise ValueError(o)
            if not INT_MIN <= r <= INT_MAX:
                raise OverflowError(f"{dst} {o} {src}: {r}")
            env[dst] = r
        return env


def latlon_program():
    p = Prog()
    # #x #z hold the player's block X/Z
    p.op("#q", "=", "#z")
    p.op("#q", "/=", 100)
    p.op("#r", "=", "#z")
    p.op("#r", "%=", 100)
    p.op("#u", "=", "#x")
    p.op("#u", "/=", 100)
    p.op("#v", "=", "#x")
    p.op("#v", "%=", 100)
    p.op("#a", "=", "#x")
    p.op("#a", "/=", 10)
    p.op("#b", "=", "#z")
    p.op("#b", "/=", 10)
    p.op("#aa", "=", "#a")
    p.op("#aa", "*=", "#a")
    p.op("#aa", "/=", 1000)
    p.op("#ab", "=", "#a")
    p.op("#ab", "*=", "#b")
    p.op("#ab", "/=", 1000)
    p.op("#bb", "=", "#b")
    p.op("#bb", "*=", "#b")
    p.op("#bb", "/=", 1000)
    for name, k in (("#lat", 0), ("#lon", 1)):
        c = fit(k)
        p.set(name, round(c["1"]))
        # linear terms: q*100 + r metres
        for var, hi, lo in (("z", "#q", "#r"), ("x", "#u", "#v")):
            k1, d1 = ratio(c[var] * 100, 80000)
            p.op("#t", "=", hi)
            p.op("#t", "*=", k1)
            p.op("#t", "/=", d1)
            p.op(name, "+=", "#t")
            k2, d2 = ratio(c[var], 20000)
            p.op("#t", "=", lo)
            p.op("#t", "*=", k2)
            p.op("#t", "/=", d2)
            p.op(name, "+=", "#t")
        # quadratic terms on 10 m units / 1000: X² = 1e5 * #aa (approx)
        for key, var in (("xx", "#aa"), ("xz", "#ab"), ("zz", "#bb")):
            k, d = ratio(c[key] * 1e5, 3000)
            if k == 0:
                continue
            p.op("#t", "=", var)
            p.op("#t", "*=", k)
            p.op("#t", "/=", d)
            p.op(name, "+=", "#t")
    # to 1e-4 degrees, split into integer part and four digits
    for name, pre in (("#lat", "la"), ("#lon", "lo")):
        p.op(name, "/=", 100)
        p.op(f"#{pre}i", "=", name)
        p.op(f"#{pre}i", "/=", 10000)
        p.op(f"#{pre}f", "=", name)
        p.op(f"#{pre}f", "%=", 10000)
        for i, div in enumerate((1000, 100, 10, 1)):
            p.op(f"#{pre}{i + 1}", "=", f"#{pre}f")
            p.op(f"#{pre}{i + 1}", "/=", div)
            p.op(f"#{pre}{i + 1}", "%=", 10)
    # check against the exact projection over the whole area
    worst = 0.0
    for x in range(-150000, 150001, 2500):
        for z in range(-30000, 230001, 2500):
            env = p.run({"#x": x, "#z": z})
            la = env["#lai"] + env["#laf"] / 1e4
            lo = env["#loi"] + env["#lof"] / 1e4
            tla, tlo = jpproj.mc_to_ll(x + 0.5, z + 0.5)
            worst = max(worst, abs(la - tla) * 111000, abs(lo - tlo) * 111000 * math.cos(math.radians(tla)))
    print(f"lat/lon program: {len(p.ops)} operations, max error {worst:.0f} m over the area")
    assert worst < 60, worst
    return p


# ---------------------------------------------------------------- text helpers

def tj(v):
    return json.dumps(v, ensure_ascii=False)


def txt(s, color=None, bold=False):
    d = {"text": s}
    if color:
        d["color"] = color
    if bold:
        d["bold"] = True
    return d


def btn(label, cmd, hover, color="aqua", suggest=False):
    return {
        "text": f"[{label}]",
        "color": color,
        "click_event": {"action": "suggest_command" if suggest else "run_command", "command": cmd},
        "hover_event": {"action": "show_text", "value": hover},
    }


def link(label, url, hover):
    return {"text": f"[{label}]", "color": "aqua", "underlined": True, "click_event": {"action": "open_url", "url": url}, "hover_event": {"action": "show_text", "value": hover}}


def tellraw(parts, who="@s"):
    return f"tellraw {who} {tj(parts)}"


# ---------------------------------------------------------------- pack

def write(path, lines):
    full = os.path.join(FN, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines) + "\n")


def main():
    probe = "--probe" in sys.argv
    places = load_places(probe)
    cfg = {}
    if os.path.exists(os.path.join(ROOT, "release.json")):
        cfg = json.load(open(os.path.join(ROOT, "release.json"), encoding="utf-8"))
    web = cfg.get("pages_url", "")
    if "OWNER" in web:
        web = ""
    prog = latlon_program()

    if os.path.exists(PACK):
        shutil.rmtree(PACK)
    os.makedirs(FN)
    with open(os.path.join(PACK, "pack.mcmeta"), "w", encoding="utf-8", newline="\n") as f:
        f.write(tj({"pack": {"description": "日本1mワールド 便利ツール / Japan 1m world tools — /function jp:menu", "min_format": [94, 1], "max_format": [94, 1]}}) + "\n")
    for tag, fn in (("load", "jp:load"), ("tick", "jp:tick")):
        d = os.path.join(PACK, "data", "minecraft", "tags", "function")
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, f"{tag}.json"), "w", encoding="utf-8", newline="\n") as f:
            f.write(tj({"values": [fn]}) + "\n")

    hud_ops = prog.emit()  # registers the constants used by the program
    consts = sorted(prog.consts | {10})
    write("load.mcfunction", [
        "# objectives: jp_t temporaries, jp_c constants, jp_lang 0 = 日本語 / 1 = English",
        "scoreboard objectives add jp_t dummy",
        "scoreboard objectives add jp_c dummy",
        "scoreboard objectives add jp_lang dummy",
        *[f"scoreboard players set {prog.const(v)} jp_c {v}" for v in consts],
    ])
    write("tick.mcfunction", [
        "execute as @a[tag=!jp_seen] run function jp:welcome",
        "scoreboard players add #tick jp_t 1",
        "execute if score #tick jp_t matches 10.. run function jp:tick10",
    ])
    write("tick10.mcfunction", [
        "scoreboard players set #tick jp_t 0",
        "execute as @a[tag=jp_hud] run function jp:hud_show",
    ])
    write("welcome.mcfunction", [
        "tag @s add jp_seen",
        tellraw([txt("日本1mワールド", "gold", True), txt(" — "), btn("メニュー", "/function jp:menu", "クリックでメニューを開く"),
                 txt("  /function jp:menu   "), btn("English", "/function jp:lang_en", "Show the menu in English")]),
    ])

    # ---- HUD: elevation and approximate latitude / longitude
    hud = [
        "execute store result score #x jp_t run data get entity @s Pos[0]",
        "execute store result score #z jp_t run data get entity @s Pos[2]",
        "execute store result score #elev jp_t run data get entity @s Pos[1]",
        "scoreboard players add #elev jp_t 1759",
        *hud_ops,
    ]
    sc = lambda n: {"score": {"name": n, "objective": "jp_t"}}  # noqa: E731
    digits = lambda pre: [sc(f"#{pre}i"), txt("."), sc(f"#{pre}1"), sc(f"#{pre}2"), sc(f"#{pre}3"), sc(f"#{pre}4")]  # noqa: E731
    hud.append("execute if score @s jp_lang matches 1 run return run title @s actionbar " + tj([txt("Elevation ", "gray"), sc("#elev"), txt(" m   ", "gray"), *digits("la"), txt("N  ", "gray"), *digits("lo"), txt("E", "gray")]))
    hud.append("title @s actionbar " + tj([txt("標高 ", "gray"), sc("#elev"), txt(" m   北緯 ", "gray"), *digits("la"), txt("  東経 ", "gray"), *digits("lo")]))
    write("hud_show.mcfunction", hud)

    # ---- simple tools (each with ja/en messages)
    def say(ja, en, color="yellow"):
        return [
            "execute if score @s jp_lang matches 1 run return run " + tellraw([txt(en, color)]),
            tellraw([txt(ja, color)]),
        ]

    def toggle(name, tag, on_cmds, off_cmds, ja_on, en_on, ja_off, en_off):
        write(f"{name}.mcfunction", [f"execute if entity @s[tag={tag}] run return run function jp:{name}_off", f"tag @s add {tag}", *on_cmds, *say(ja_on, en_on)])
        write(f"{name}_off.mcfunction", [f"tag @s remove {tag}", *off_cmds, *say(ja_off, en_off)])

    toggle("hud", "jp_hud", [], ["title @s actionbar \"\""],
           "現在地表示 ON（標高と緯度経度。緯度経度は約30 m の精度）", "Position display ON (elevation, latitude/longitude to about 30 m)",
           "現在地表示 OFF", "Position display OFF")
    toggle("nv", "jp_nv", ["effect give @s night_vision infinite 0 true"], ["effect clear @s night_vision"],
           "夜間視覚 ON", "Night vision ON", "夜間視覚 OFF", "Night vision OFF")
    toggle("step", "jp_step", ["attribute @s minecraft:step_height base set 1.05"], ["attribute @s minecraft:step_height base reset"],
           "1 段の段差を自動で上る ON", "Auto step-up (1 block) ON", "自動で上る OFF", "Auto step-up OFF")
    write("day.mcfunction", [
        "gamerule minecraft:advance_time false", "gamerule minecraft:advance_weather false", "time set noon", "weather clear",
        *say("昼・晴れに固定しました（元に戻す: /function jp:time_normal）", "Fixed to midday and clear weather (undo: /function jp:time_normal)"),
    ])
    write("time_normal.mcfunction", [
        "gamerule minecraft:advance_time true", "gamerule minecraft:advance_weather true",
        *say("時間と天気が進むようにしました", "Time and weather run normally again"),
    ])
    write("fall.mcfunction", [
        "execute store result score #fd jp_t run gamerule minecraft:fall_damage",
        "execute if score #fd jp_t matches 1 run gamerule minecraft:fall_damage false",
        "execute if score #fd jp_t matches 0 run gamerule minecraft:fall_damage true",
        "execute if score #fd jp_t matches 1 run return run function jp:fall_msg_off",
        "function jp:fall_msg_on",
    ])
    write("fall_msg_off.mcfunction", say("落下ダメージ OFF", "Fall damage OFF"))
    write("fall_msg_on.mcfunction", say("落下ダメージ ON", "Fall damage ON"))
    write("elytra.mcfunction", [
        "give @s elytra[unbreakable={}] 1",
        "give @s firework_rocket[fireworks={flight_duration:3}] 64",
        *say("エリトラとロケット花火を渡しました。胸に装備し、落下中にジャンプで滑空、花火で加速します",
             "Gave an elytra and rockets: wear it, jump while falling to glide, use rockets to boost"),
    ])
    missing_ja = "その場所のタイルはダウンロードされていないようです（タイル選択ページで追加できます）"
    missing_en = "That area does not seem to be downloaded (add its tile on the tile selection page)"
    write("surface.mcfunction", [
        "execute store success score #ok jp_t run spreadplayers ~ ~ 0 1 false @s",
        "execute if score #ok jp_t matches 0 run return run function jp:missing",
        *say("真下の一番上のブロックに降りました", "Moved down onto the top block below you"),
    ])
    write("goto.mcfunction", [
        "# usage: /function jp:goto {x:-10094,z:113663}  (lands on the top block at X, Z)",
        "$execute store success score #ok jp_t run spreadplayers $(x) $(z) 0 1 false @s",
        "execute if score #ok jp_t matches 0 run return run function jp:missing",
        "$execute if score @s jp_lang matches 1 run return run " + tellraw([txt("Moved to X $(x) Z $(z)", "yellow")]),
        "$" + tellraw([txt("X $(x) Z $(z) に移動しました", "yellow")]),
    ])
    write("missing.mcfunction", say(missing_ja, missing_en, "red"))
    # after a landmark teleport: every generated column of the world has bedrock at
    # the bottom (Y -2032); a column without it was never downloaded -> back to spawn
    write("check_ground.mcfunction", [
        "execute as @a[tag=jp_chk] at @s unless block ~ -2032 ~ minecraft:bedrock run function jp:not_downloaded",
        "tag @a remove jp_chk",
    ])
    write("not_downloaded.mcfunction", [
        "effect clear @s slow_falling",
        f"tp @s {SPAWN[0]} {SPAWN[1]} {SPAWN[2]}",
        *say(missing_ja + "。スポーン地点（富士山頂）に戻しました", missing_en + ". Sent you back to spawn (summit of Mt. Fuji).", "red"),
    ])
    write("lang_en.mcfunction", ["scoreboard players set @s jp_lang 1", "function jp:menu"])
    write("lang_ja.mcfunction", ["scoreboard players set @s jp_lang 0", "function jp:menu"])

    # ---- landmarks
    for p in places:
        high = p["y"] > 1500
        y = min(p["y"] + (2 if high else 40), TOP_Y - 3)
        write(f"place/{p['id']}.mcfunction", [
            f"# {p['ja']} / {p['en']}  ({p['lat']}, {p['lon']})",
            f"tp @s {p['x']} {y} {p['z']}",
            "effect give @s slow_falling 30 0 true",
            "tag @s add jp_chk",
            "schedule function jp:check_ground 40t replace",
            "execute if score @s jp_lang matches 1 run return run " + tellraw([txt(p["en"], "gold"), txt(f"  X {p['x']}  Z {p['z']}  (ground Y {p['y']})", "gray")]),
            tellraw([txt(p["ja"], "gold"), txt(f"  X {p['x']}  Z {p['z']}（地表 Y {p['y']}）", "gray")]),
        ])
    for lang, (ttl, back) in (("ja", ("主要地点へ移動（クリック）", "メニューに戻る")), ("en", ("Go to a landmark (click)", "Back to menu"))):
        lines = [tellraw([txt("— " + ttl + " —", "gold", True)])]
        for gid, gja, gen in GROUPS:
            row = [txt((gja if lang == "ja" else gen) + ": ", "yellow")]
            for p in [q for q in places if q["group"] == gid]:
                row.append(btn(p[lang], f"/function jp:place/{p['id']}", f"X {p['x']}  Z {p['z']}", "aqua"))
                row.append(txt(" "))
            lines.append(tellraw(row))
        lines.append(tellraw([btn(back, "/function jp:menu", back, "green")]))
        write(f"places_{lang}.mcfunction", lines)
    write("places.mcfunction", ["execute if score @s jp_lang matches 1 run return run function jp:places_en", "function jp:places_ja"])

    # ---- menu
    def menu(lang):
        J = lang == "ja"
        L = (lambda ja, en: ja if J else en)
        rows = [
            [txt(L("— 日本1mワールド メニュー —", "— Japan 1m world menu —"), "gold", True)],
            [btn(L("主要地点へ移動", "Landmarks"), "/function jp:places", L("富士山・東京駅など 44 か所", "Mt. Fuji, Tokyo Station and 42 more")),
             txt(" "), btn(L("座標で移動", "Go to X/Z"), "/function jp:goto {x:0,z:0}", L("X と Z を書き換えて Enter（地表に着地）", "Edit X and Z, then Enter (lands on the surface)"), suggest=True),
             txt(" "), btn(L("真下の地表へ", "Drop to ground"), "/function jp:surface", L("真下の一番上のブロックに移動", "Move onto the top block below you"))],
            [btn(L("現在地表示", "Position HUD"), "/function jp:hud", L("標高と緯度経度を画面下に表示（ON/OFF）", "Show elevation and lat/lon above the hotbar (on/off)")),
             txt(" "), btn(L("夜間視覚", "Night vision"), "/function jp:nv", L("暗い所でも見える（ON/OFF）", "See in the dark (on/off)")),
             txt(" "), btn(L("昼・晴れに固定", "Always day"), "/function jp:day", L("時間と天気を止める", "Stop time and weather")),
             txt(" "), btn(L("時間を戻す", "Normal time"), "/function jp:time_normal", L("時間と天気を進める", "Let time and weather run"))],
            [btn(L("段差を自動で上る", "Auto step-up"), "/function jp:step", L("1 ブロックの段差をジャンプせずに上る（ON/OFF）", "Walk up 1-block steps without jumping (on/off)")),
             txt(" "), btn(L("落下ダメージ", "Fall damage"), "/function jp:fall", L("落下ダメージの ON/OFF（ワールド全体）", "Toggle fall damage (whole world)")),
             txt(" "), btn(L("エリトラセット", "Elytra kit"), "/function jp:elytra", L("エリトラと花火 64 個", "An elytra and 64 rockets"))],
        ]
        last = [btn("English" if J else "日本語", "/function jp:lang_en" if J else "/function jp:lang_ja", "Switch language / 言語の切り替え", "green")]
        if web:
            last += [txt(" "), link(L("Web の移動コマンド作成", "Web teleport tool"), web + "teleport.html", L("地図から移動コマンドを作る", "Make teleport commands on a map"))]
        rows.append(last)
        write(f"menu_{lang}.mcfunction", [tellraw(r) for r in rows])

    menu("ja")
    menu("en")
    write("menu.mcfunction", ["execute if score @s jp_lang matches 1 run return run function jp:menu_en", "function jp:menu_ja"])
    n = sum(len(fs) for _, _, fs in os.walk(FN))
    print(f"data pack: {n} functions -> {PACK}; {len(places)} landmarks")


if __name__ == "__main__":
    main()
