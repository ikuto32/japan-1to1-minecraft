// Tile selection page: pick tiles on the map, get the download commands.
"use strict";

const PRESETS = [
  { ja: "富士山", en: "Mt. Fuji", lat: 35.36, lon: 138.73, km: 14 },
  { ja: "富士五湖", en: "Fuji Five Lakes", lat: 35.49, lon: 138.7, km: 14 },
  { ja: "静岡市", en: "Shizuoka", lat: 34.98, lon: 138.4, km: 9 },
  { ja: "牧之原・静岡空港", en: "Makinohara & airport", lat: 34.79, lon: 138.17, km: 7 },
  { ja: "浜松・浜名湖", en: "Hamamatsu & Lake Hamana", lat: 34.73, lon: 137.65, km: 10 },
  { ja: "箱根・熱海", en: "Hakone & Atami", lat: 35.2, lon: 139.05, km: 10 },
  { ja: "伊豆半島", en: "Izu Peninsula", lat: 34.85, lon: 138.95, km: 26 },
  { ja: "甲府", en: "Kofu", lat: 35.66, lon: 138.57, km: 7 },
  { ja: "東京都心", en: "Central Tokyo", lat: 35.68, lon: 139.75, km: 11 },
  { ja: "横浜・鎌倉", en: "Yokohama & Kamakura", lat: 35.38, lon: 139.58, km: 12 },
];

let manifest = null;
const layers = new Map(); // id -> polygon
const selected = new Set();
let map;

function tileStyle(e) {
  const sel = selected.has(e.id);
  const landShare = e.inside > 0 ? e.land / e.inside : 0;
  return {
    color: sel ? "#e8782c" : "#666",
    weight: sel ? 2 : 0.6,
    fillColor: sel ? "#e8782c" : landShare > 0.02 ? "#5a8f3a" : "#3b7fb5",
    fillOpacity: sel ? 0.38 : landShare > 0.02 ? 0.04 + 0.12 * landShare : 0.05,
  };
}

function refresh() {
  for (const e of manifest.tiles) layers.get(e.id).setStyle(tileStyle(e));
  const tiles = manifest.tiles.filter((e) => selected.has(e.id));
  const ids = tiles.map((e) => e.id);
  const base = manifest.base ? manifest.base.size : 4e7;
  const dl = tiles.reduce((s, e) => s + e.size, 0) + (tiles.length ? base : 0);
  const raw = tiles.reduce((s, e) => s + (e.raw || 0), 0);
  document.getElementById("nSel").textContent = tiles.length;
  document.getElementById("dlSize").textContent = tiles.length ? fmtGB(dl) : "–";
  document.getElementById("diskSize").textContent = tiles.length ? fmtGB(raw + base) : "–";
  document.getElementById("est").textContent = tiles.some((e) => e.estimated) ? t("estimated") : "";
  const url = manifest.base_url;
  const win = document.getElementById("cmdWin");
  const unix = document.getElementById("cmdUnix");
  if (!ids.length) {
    win.textContent = unix.textContent = t("none_selected");
  } else {
    const all = ids.length === manifest.tiles.length;
    win.textContent = `curl.exe -L -o download.ps1 ${url}download.ps1\npowershell -ExecutionPolicy Bypass -File download.ps1 -Tiles ${all ? "all" : ids.join(",")}`;
    unix.textContent = `curl -LO ${url}download.sh\nbash download.sh ${all ? "all" : ids.join(" ")}`;
  }
  const links = document.getElementById("links");
  links.innerHTML = "";
  if (ids.length) {
    const add = (file, size) => {
      const a = document.createElement("a");
      a.href = url + file;
      a.textContent = `${file} (${fmtGB(size)})`;
      links.appendChild(a);
    };
    if (manifest.base) add(manifest.base.file, manifest.base.size);
    tiles.forEach((e) => add(e.file, e.size));
  }
  try { history.replaceState(null, "", ids.length ? "#tiles=" + ids.join(",") : location.pathname); } catch (e) { /* file:// */ }
  try { localStorage.setItem("jp1m_tiles", ids.join(",")); } catch (e) { /* ignore */ }
}

function toggle(id, on) {
  if (on === undefined) on = !selected.has(id);
  if (on) selected.add(id); else selected.delete(id);
}

function tilesNear(lat, lon, km) {
  const [cx, cz] = JPProj.llToMc(lat, lon);
  const r = km * 1000;
  const T = manifest.tile_size;
  return manifest.tiles.filter((e) => {
    const dx = Math.max(e.x - cx, 0, cx - (e.x + T));
    const dz = Math.max(e.z - cz, 0, cz - (e.z + T));
    return dx * dx + dz * dz <= r * r;
  });
}

function renderPresets() {
  const box = document.getElementById("presets");
  box.innerHTML = "";
  for (const p of PRESETS) {
    const b = document.createElement("button");
    b.textContent = p[LANG] || p.en;
    b.addEventListener("click", () => {
      const ts = tilesNear(p.lat, p.lon, p.km);
      const allOn = ts.every((e) => selected.has(e.id));
      ts.forEach((e) => toggle(e.id, !allOn));
      refresh();
      map.fitBounds(L.latLngBounds(ts.flatMap((e) => tileLatLngs(e.x, e.z, manifest.tile_size))), { maxZoom: 11 });
    });
    box.appendChild(b);
  }
}

function tipHtml(e) {
  const landPct = Math.round((e.land / Math.max(e.inside, 1e-9)) * 100);
  return `<b>${e.id}</b><br>${t("land")} ${landPct}% · ${t("size")} ${fmtGB(e.size)}${e.estimated ? "*" : ""}<br>${t("max_elev")} ${Math.round(e.elev_max)} m`;
}

function setupBoxSelect() {
  let start = null;
  let rect = null;
  map.on("mousedown", (ev) => {
    if (!ev.originalEvent.shiftKey) return;
    start = ev.latlng;
    map.dragging.disable();
    rect = L.rectangle([start, start], { color: "#e8782c", weight: 1, dashArray: "3 3", fillOpacity: 0.08, interactive: false }).addTo(map);
  });
  map.on("mousemove", (ev) => { if (start) rect.setBounds([start, ev.latlng]); });
  const finish = () => {
    if (!start) return;
    const b = rect.getBounds();
    rect.remove();
    start = null;
    map.dragging.enable();
    const ts = manifest.tiles.filter((e) => b.contains(L.latLng(JPProj.mcToLl(e.x + manifest.tile_size / 2, e.z + manifest.tile_size / 2))));
    const allOn = ts.length && ts.every((e) => selected.has(e.id));
    ts.forEach((e) => toggle(e.id, !allOn));
    refresh();
  };
  map.on("mouseup", finish);
  document.addEventListener("mouseup", finish);
}

async function search() {
  const q = document.getElementById("q").value.trim();
  const out = document.getElementById("qRes");
  if (!q) return;
  out.textContent = "…";
  let res = [];
  try { res = await gsiSearch(q); } catch (e) { res = []; }
  if (!res.length) { out.textContent = t("not_found"); return; }
  out.innerHTML = "";
  res.slice(0, 6).forEach((r, i) => {
    const a = document.createElement("a");
    a.href = "#";
    a.textContent = r.title + (r.inside ? "" : LANG === "ja" ? "（範囲外）" : " (outside the world)");
    a.style.display = "block";
    a.addEventListener("click", (ev) => { ev.preventDefault(); map.setView([r.lat, r.lon], 12); });
    out.appendChild(a);
    if (i === 0) map.setView([r.lat, r.lon], 12);
  });
}

async function main() {
  initHeader("tiles.html");
  map = makeMap("map");
  try {
    manifest = await loadJson("data/tiles.json");
  } catch (e) {
    document.getElementById("cmdWin").textContent = "data/tiles.json: " + e.message;
    return;
  }
  document.getElementById("relLink").href = `https://github.com/${manifest.repo}/releases/tag/${manifest.tag}`;
  for (const e of manifest.tiles) {
    const poly = L.polygon(tileLatLngs(e.x, e.z, manifest.tile_size), tileStyle(e)).addTo(map);
    poly.bindTooltip(() => tipHtml(e), { sticky: true, className: "tile-tip" });
    poly.on("click", (ev) => {
      if (ev.originalEvent.shiftKey) return;
      toggle(e.id);
      refresh();
    });
    layers.set(e.id, poly);
  }
  // selection from the URL (#tiles=a,b) or the last visit
  let init = "";
  const m = location.hash.match(/tiles=([^&]+)/);
  if (m) init = decodeURIComponent(m[1]);
  else { try { init = localStorage.getItem("jp1m_tiles") || ""; } catch (e) { /* ignore */ } }
  init.split(",").filter((id) => layers.has(id)).forEach((id) => selected.add(id));
  setupBoxSelect();
  renderPresets();
  document.getElementById("clear").addEventListener("click", () => { selected.clear(); refresh(); });
  document.getElementById("land").addEventListener("click", () => {
    manifest.tiles.filter((e) => e.land / Math.max(e.inside, 1e-9) > 0.02).forEach((e) => selected.add(e.id));
    refresh();
  });
  document.getElementById("qBtn").addEventListener("click", search);
  document.getElementById("q").addEventListener("keydown", (ev) => { if (ev.key === "Enter") search(); });
  document.querySelectorAll("button.copy").forEach((b) => b.addEventListener("click", () => copyText(document.getElementById(b.dataset.for).textContent, b)));
  document.getElementById("saveTxt").addEventListener("click", () => {
    const blob = new Blob([[...selected].sort().join("\n") + "\n"], { type: "text/plain" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = "tiles.txt";
    a.click();
    URL.revokeObjectURL(a.href);
  });
  document.addEventListener("langchange", () => { renderPresets(); refresh(); });
  refresh();
}

main();
