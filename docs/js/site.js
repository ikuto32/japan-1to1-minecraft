// Shared helpers for the pages: language, manifest, map, GSI search/elevation.
"use strict";

const I18N = {
  ja: {
    brand: "日本 1:1 Minecraft ワールド",
    nav_home: "ホーム", nav_tiles: "タイル選択", nav_tp: "移動コマンド",
    lang: "English",
    search_ph: "地名・住所・駅名（例: 静岡駅）", search: "検索", not_found: "見つかりませんでした",
    // tiles page
    tiles_intro: "地図の四角（タイル、10,240 ブロック ≒ 10 km 四方）をクリックして選び、下のコマンドでダウンロードします。Shift を押しながらドラッグすると範囲で選べます。",
    presets: "よく使う範囲", sel_title: "選択中", tiles_unit: "タイル",
    dl_size: "ダウンロード", disk_size: "展開後",
    estimated: "（概算）",
    clear: "選択を解除", select_land: "陸のタイルをすべて",
    win_cmd: "Windows（PowerShell かコマンドプロンプトに貼り付け）",
    unix_cmd: "macOS / Linux（ターミナルに貼り付け）",
    copy: "コピー", copied: "コピーしました",
    tiles_txt: "tiles.txt を保存", manual: "手動でダウンロードする",
    manual_note: "base の zip を saves フォルダの中に作った japan_dem_1m フォルダに展開し、各タイルの zip を同じフォルダに展開します（region フォルダに .mca が入ります）。",
    base_note: "base（level.dat・データパック・スポーン地点の富士山頂付近）は自動で一緒にダウンロードされます。",
    after_note: "完了したら Minecraft Java Edition 1.21.11 のシングルプレイから開きます。後からタイルを追加するときは、同じコマンドに新しいタイルを入れて実行します（取得済みのタイルは飛ばします）。",
    none_selected: "タイルを選んでください",
    land: "陸", sea: "海", max_elev: "最高標高", size: "サイズ",
    release_page: "リリースページ",
    // teleport page
    tp_intro: "地図をクリックするか地名を検索すると、その場所へ移動するゲーム内コマンドを作ります。チャット欄（T キー）に貼り付けて実行します。",
    landmarks: "主要地点", choose: "選んでください",
    latlon: "緯度・経度", mc_xz: "ゲームの X・Z（F3 で表示）", go: "地図で表示",
    point: "選んだ地点", elev: "標高", ground_y: "地表の Y", tile: "タイル",
    elev_loading: "標高を取得中…", elev_na: "取得できませんでした（海・範囲外）",
    outside: "この地点はワールドの範囲外です（北緯 34〜36°・東経 137〜140°）",
    height: "到着する高さ", h_ground: "地表", h_30: "上空 30 m", h_100: "上空 100 m",
    cmd_goto: "データパックで地表に着地（おすすめ）",
    cmd_tp: "/tp（標高から計算した高さへ）",
    cmd_spread: "バニラのコマンドで地表に着地",
    tp_note: "地表の Y は国土地理院の標高 API の値から計算しているため、建物の上などでは 1〜2 ブロック違うことがあります。データパックのコマンドは、その位置の一番上のブロックに着地します。",
    tile_missing_note: "移動先のタイルをダウンロードしていないと、何もない場所に出ます（データパックのコマンドは移動しません）。",
    // home
    home_lead: "国土地理院の 1 m 標高データと、道路・鉄道・建物・植生のオープンデータから作った、静岡県・山梨県・東京都などを 1 ブロック = 1 m で再現した Minecraft Java Edition 1.21.11 のワールドです。",
    home_tiles: "範囲を選んでダウンロード", home_tiles_p: "全体は約 1.1 TB（ダウンロードは約 470 GB）あるので、遊びたい場所のタイル（約 10 km 四方）だけを選んで落とせます。",
    home_tp: "移動コマンドを作る", home_tp_p: "地図や地名から、その場所へ移動する /tp コマンドを作ります。",
    home_game: "ゲーム内メニュー", home_game_p: "チャットで /function jp:menu と入力すると、主要地点への移動、標高・緯度経度の表示、夜間視覚などが使えます。",
    open: "開く", gallery: "上から見た確認画像（左: ワールド、右: 国土地理院の航空写真）",
    req: "必要なもの: Minecraft Java Edition 1.21.11（バージョンを合わせてください）、タイル 1 枚あたり約 0.2〜1.5 GB のディスク。",
    license_link: "出典・ライセンス", code_license: "ソースコード: MIT ライセンス",
  },
  en: {
    brand: "Japan 1:1 Minecraft World",
    nav_home: "Home", nav_tiles: "Tiles", nav_tp: "Teleport",
    lang: "日本語",
    search_ph: "Place, address or station (e.g. 静岡駅, Tokyo Station)", search: "Search", not_found: "Nothing found",
    tiles_intro: "Click the squares (tiles, 10,240 blocks ≈ 10 km across) to select them, then download them with the command below. Shift-drag selects an area.",
    presets: "Common areas", sel_title: "Selected", tiles_unit: "tiles",
    dl_size: "Download", disk_size: "On disk",
    estimated: " (estimate)",
    clear: "Clear", select_land: "All land tiles",
    win_cmd: "Windows (paste into PowerShell or Command Prompt)",
    unix_cmd: "macOS / Linux (paste into a terminal)",
    copy: "Copy", copied: "Copied",
    tiles_txt: "Save tiles.txt", manual: "Download manually",
    manual_note: "Unzip the base zip into a new folder japan_dem_1m inside your saves folder, then unzip every tile zip into the same folder (the .mca files go to its region folder).",
    base_note: "The base package (level.dat, data packs and the spawn area at the summit of Mt. Fuji) is always downloaded too.",
    after_note: "Then open it in Minecraft Java Edition 1.21.11, Singleplayer. To add tiles later, run the same command with the new tiles (tiles you already have are skipped).",
    none_selected: "Select some tiles",
    land: "land", sea: "sea", max_elev: "highest point", size: "size",
    release_page: "Release page",
    tp_intro: "Click the map or search for a place to get the in-game command that takes you there. Paste it into chat (T key).",
    landmarks: "Landmarks", choose: "Choose…",
    latlon: "Latitude, longitude", mc_xz: "In-game X, Z (shown by F3)", go: "Show",
    point: "Selected point", elev: "Elevation", ground_y: "Ground Y", tile: "Tile",
    elev_loading: "Getting the elevation…", elev_na: "Not available (sea or outside Japan)",
    outside: "This point is outside the world (latitude 34–36°N, longitude 137–140°E)",
    height: "Arrive at", h_ground: "ground", h_30: "30 m above", h_100: "100 m above",
    cmd_goto: "Land on the surface with the data pack (recommended)",
    cmd_tp: "/tp (to the height from the elevation)",
    cmd_spread: "Land on the surface with vanilla commands",
    tp_note: "The ground Y comes from the GSI elevation API, so on buildings it can be 1–2 blocks off. The data pack command lands you on the top block at that position.",
    tile_missing_note: "If you have not downloaded the destination tile, you arrive in empty space (the data pack command will not move you).",
    home_lead: "A Minecraft Java Edition 1.21.11 world of Shizuoka, Yamanashi, Tokyo and the surrounding area at 1 block = 1 m, made from the 1 m elevation data of the Geospatial Information Authority of Japan and open data for roads, railways, buildings and vegetation.",
    home_tiles: "Pick an area and download", home_tiles_p: "The whole world is about 1.1 TB (470 GB of downloads), so you download only the tiles (about 10 km square) you want to play in.",
    home_tp: "Make teleport commands", home_tp_p: "Get the /tp command for any place from the map or a place name.",
    home_game: "In-game menu", home_game_p: "Type /function jp:menu in chat for landmarks, an elevation and latitude/longitude display, night vision and more.",
    open: "Open", gallery: "Top-down check images (left: the world, right: GSI aerial photo)",
    req: "You need Minecraft Java Edition 1.21.11 (match the version) and about 0.2–1.5 GB of disk per tile.",
    license_link: "Sources and licences", code_license: "Source code: MIT License",
  },
};

let LANG = (() => {
  try {
    const s = localStorage.getItem("jp1m_lang");
    if (s === "ja" || s === "en") return s;
  } catch (e) { /* storage unavailable */ }
  return (navigator.language || "").toLowerCase().startsWith("ja") ? "ja" : "en";
})();

function t(k) { return (I18N[LANG] && I18N[LANG][k]) || I18N.en[k] || k; }

function applyI18n() {
  document.documentElement.lang = LANG;
  document.querySelectorAll("[data-i18n]").forEach((el) => { el.textContent = t(el.dataset.i18n); });
  document.querySelectorAll("[data-i18n-ph]").forEach((el) => { el.placeholder = t(el.dataset.i18nPh); });
  document.querySelectorAll("[data-lang]").forEach((el) => { el.hidden = el.dataset.lang !== LANG; });
}

function setLang(l) {
  LANG = l;
  try { localStorage.setItem("jp1m_lang", l); } catch (e) { /* ignore */ }
  applyI18n();
  document.dispatchEvent(new Event("langchange"));
}

function initHeader(cur) {
  const h = document.querySelector("header.top");
  h.innerHTML =
    '<span class="brand" data-i18n="brand"></span>' +
    "<nav>" +
    '<a href="index.html" data-i18n="nav_home"></a>' +
    '<a href="tiles.html" data-i18n="nav_tiles"></a>' +
    '<a href="teleport.html" data-i18n="nav_tp"></a>' +
    "</nav>" +
    '<button id="langBtn" data-i18n="lang"></button>';
  h.querySelectorAll("nav a").forEach((a) => { if (a.getAttribute("href") === cur) a.classList.add("cur"); });
  h.querySelector("#langBtn").addEventListener("click", () => setLang(LANG === "ja" ? "en" : "ja"));
  applyI18n();
}

async function loadJson(path) {
  const r = await fetch(path, { cache: "no-cache" });
  if (!r.ok) throw new Error(path + ": " + r.status);
  return r.json();
}

function fmtGB(bytes) {
  if (bytes >= 1e9) return (bytes / 1e9).toFixed(bytes >= 1e11 ? 0 : 1) + " GB";
  return Math.max(1, Math.round(bytes / 1e6)) + " MB";
}

function makeMap(el, view) {
  const gsiAttr = '<a href="https://maps.gsi.go.jp/development/ichiran.html" target="_blank" rel="noopener">地理院タイル</a>';
  const pale = L.tileLayer("https://cyberjapandata.gsi.go.jp/xyz/pale/{z}/{x}/{y}.png", { attribution: gsiAttr, maxZoom: 18 });
  const std = L.tileLayer("https://cyberjapandata.gsi.go.jp/xyz/std/{z}/{x}/{y}.png", { attribution: gsiAttr, maxZoom: 18 });
  const photo = L.tileLayer("https://cyberjapandata.gsi.go.jp/xyz/seamlessphoto/{z}/{x}/{y}.jpg", { attribution: gsiAttr, maxZoom: 18 });
  const osm = L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", {
    attribution: '&copy; <a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noopener">OpenStreetMap contributors</a>', maxZoom: 19,
  });
  const map = L.map(el, { boxZoom: false, worldCopyJump: false, layers: [pale] }).setView(view || [35.25, 138.7], 8);
  L.control.layers({ "地理院 淡色 / GSI pale": pale, "地理院 標準 / GSI standard": std, "地理院 写真 / GSI photo": photo, "OpenStreetMap": osm }, null, { position: "topright" }).addTo(map);
  L.control.scale({ imperial: false }).addTo(map);
  // the world's area (1st meshes 5137–5339)
  const a = JPProj.AREA;
  L.rectangle([[a.latMin, a.lonMin], [a.latMax, a.lonMax]], { color: "#b5462f", weight: 1, dashArray: "4 4", fill: false, interactive: false }).addTo(map);
  return map;
}

/** Outline of a tile (x0, z0 Minecraft) as lat/lon points, edges subdivided. */
function tileLatLngs(x0, z0, size) {
  const pts = [];
  const steps = 4;
  const edge = (ax, az, bx, bz) => {
    for (let i = 0; i < steps; i++) {
      const f = i / steps;
      pts.push(JPProj.mcToLl(ax + (bx - ax) * f, az + (bz - az) * f));
    }
  };
  edge(x0, z0, x0 + size, z0);
  edge(x0 + size, z0, x0 + size, z0 + size);
  edge(x0 + size, z0 + size, x0, z0 + size);
  edge(x0, z0 + size, x0, z0);
  return pts;
}

/** GSI place/address search: [{title, lat, lon}]. */
async function gsiSearch(q) {
  const r = await fetch("https://msearch.gsi.go.jp/address-search/AddressSearch?q=" + encodeURIComponent(q));
  if (!r.ok) return [];
  const j = await r.json();
  const res = j.slice(0, 40).map((f) => ({ title: f.properties.title, lon: f.geometry.coordinates[0], lat: f.geometry.coordinates[1] }));
  // places inside the world first (the search covers all of Japan)
  res.forEach((p) => { p.inside = inArea(p.lat, p.lon); });
  res.sort((a, b) => b.inside - a.inside);
  return res;
}

/**
 * Place search for the search boxes: the site's landmark list (data/places.json) first, then GSI results
 * whose title is or contains the words, then the rest. GSI's address search does not know every station
 * (「東京駅」 only matches addresses with 「東」), so a result is `good` (safe to jump to) only when it matches.
 * [{title, lat, lon, inside, good}]
 */
async function placeSearch(q, places = []) {
  const n = (s) => (s || "").normalize("NFKC").replace(/\s+/g, "").toLowerCase();
  const nq = n(q);
  const base = (s) => n(s).replace(/[（(].*$/, "");
  const local = places
    .filter((p) => n(p.ja).includes(nq) || n(p.en).includes(nq) || (base(p.ja).length >= 2 && nq.includes(base(p.ja))))
    .map((p) => ({ title: LANG === "ja" ? p.ja : p.en, lat: p.lat, lon: p.lon, y: p.y, inside: true, good: true }));
  let res = [];
  try { res = await gsiSearch(q); } catch (e) { res = []; }
  const rank = (r) => (n(r.title) === nq ? 0 : n(r.title).includes(nq) ? 1 : 2);
  res.sort((a, b) => rank(a) - rank(b) || b.inside - a.inside);
  res.forEach((r) => { r.good = rank(r) < 2; });
  const seen = new Set(local.map((p) => `${p.lat.toFixed(3)},${p.lon.toFixed(3)}`));
  return local.concat(res.filter((r) => !seen.has(`${r.lat.toFixed(3)},${r.lon.toFixed(3)}`)));
}

/** GSI elevation API: metres or null (sea / outside). */
async function gsiElevation(lat, lon) {
  const r = await fetch(`https://cyberjapandata2.gsi.go.jp/general/dem/scripts/getelevation.php?lon=${lon}&lat=${lat}&outtype=JSON`);
  if (!r.ok) return null;
  const j = await r.json();
  const v = parseFloat(j.elevation);
  return Number.isFinite(v) ? { h: v, src: j.hsrc } : null;
}

async function copyText(text, btn) {
  try {
    await navigator.clipboard.writeText(text);
  } catch (e) {
    const ta = document.createElement("textarea");
    ta.value = text;
    document.body.appendChild(ta);
    ta.select();
    document.execCommand("copy");
    ta.remove();
  }
  if (btn) {
    const old = btn.textContent;
    btn.textContent = t("copied");
    setTimeout(() => { btn.textContent = old; }, 1400);
  }
}

function tileIdAt(x, z) {
  const T = JPProj.TILE;
  return `x${Math.floor(x / T) * T}_z${Math.floor(z / T) * T}`;
}

function inArea(lat, lon) {
  const a = JPProj.AREA;
  return lat >= a.latMin && lat < a.latMax && lon >= a.lonMin && lon < a.lonMax;
}
