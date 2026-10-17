// Teleport command generator: a point on the map -> Minecraft commands.
"use strict";

let map, marker, places = [];
let cur = null; // {lat, lon, x, z, h (m or null), src}
let req = 0;

function cmdText() {
  if (!cur) return;
  const x = Math.floor(cur.x);
  const z = Math.floor(cur.z);
  const add = parseInt(document.getElementById("hsel").value, 10);
  const ground = cur.h == null ? null : JPProj.SEA_Y + Math.round(cur.h);
  const y = ground == null ? null : Math.min(ground + add, JPProj.TOP_Y - 2);
  document.getElementById("cGoto").textContent = `/function jp:goto {x:${x},z:${z}}`;
  document.getElementById("cTp").textContent = y == null ? "–" : `/tp @s ${x} ${y} ${z}`;
  document.getElementById("cSpread").textContent = `/spreadplayers ${x} ${z} 0 1 false @s`;
  document.getElementById("iY").textContent = ground == null ? "–" : `${ground}（${LANG === "ja" ? "立つ高さ" : "stand at"} ${ground + 1}）`;
}

async function setPoint(lat, lon, opts = {}) {
  const [x, z] = JPProj.llToMc(lat, lon);
  cur = { lat, lon, x, z, h: opts.y != null ? opts.y - JPProj.SEA_Y : null };
  if (!marker) {
    marker = L.marker([lat, lon], { draggable: true }).addTo(map);
    marker.on("dragend", () => { const p = marker.getLatLng(); setPoint(p.lat, p.lng); });
  } else marker.setLatLng([lat, lon]);
  if (opts.pan) map.setView([lat, lon], Math.max(map.getZoom(), opts.zoom || 14));
  document.getElementById("iLL").textContent = `${lat.toFixed(6)}, ${lon.toFixed(6)}`;
  document.getElementById("iXZ").textContent = `${Math.floor(x)}, ${Math.floor(z)}`;
  document.getElementById("iTile").textContent = tileIdAt(x, z);
  document.getElementById("outside").hidden = inArea(lat, lon);
  cmdText();
  if (cur.h != null) {
    document.getElementById("iElev").textContent = `${cur.h.toFixed(1)} m`;
    return;
  }
  const my = ++req;
  document.getElementById("iElev").textContent = t("elev_loading");
  let e = null;
  try { e = await gsiElevation(lat, lon); } catch (err) { e = null; }
  if (my !== req) return;
  if (e) {
    cur.h = e.h;
    document.getElementById("iElev").textContent = `${e.h.toFixed(1)} m（${e.src}）`;
  } else {
    cur.h = 0;
    document.getElementById("iElev").textContent = t("elev_na");
  }
  cmdText();
}

async function search() {
  const q = document.getElementById("q").value.trim();
  const out = document.getElementById("qRes");
  if (!q) return;
  out.textContent = "…";
  const res = await placeSearch(q, places);
  if (!res.length) { out.textContent = t("not_found"); return; }
  out.innerHTML = "";
  res.slice(0, 6).forEach((r, i) => {
    const a = document.createElement("a");
    a.href = "#";
    a.textContent = r.title + (r.inside ? "" : LANG === "ja" ? "（範囲外）" : " (outside the world)");
    a.style.display = "block";
    a.addEventListener("click", (ev) => { ev.preventDefault(); setPoint(r.lat, r.lon, { pan: true, y: r.y }); });
    out.appendChild(a);
    // jump to the first result only when it matches the words (otherwise let the user pick)
    if (i === 0 && r.good) setPoint(r.lat, r.lon, { pan: true, y: r.y });
  });
}

function renderPlaces() {
  const sel = document.getElementById("places");
  sel.innerHTML = "";
  const o = document.createElement("option");
  o.value = "";
  o.textContent = `${t("landmarks")} — ${t("choose")}`;
  sel.appendChild(o);
  for (const p of places) {
    const op = document.createElement("option");
    op.value = p.id;
    op.textContent = LANG === "ja" ? p.ja : p.en;
    sel.appendChild(op);
  }
}

async function main() {
  initHeader("teleport.html");
  map = makeMap("map", [35.36, 138.73]);
  map.on("click", (ev) => setPoint(ev.latlng.lat, ev.latlng.lng));
  try { places = await loadJson("data/places.json"); } catch (e) { places = []; }
  renderPlaces();
  document.getElementById("places").addEventListener("change", (ev) => {
    const p = places.find((q) => q.id === ev.target.value);
    if (p) setPoint(p.lat, p.lon, { pan: true, y: p.y, zoom: 15 });
  });
  document.getElementById("qBtn").addEventListener("click", search);
  document.getElementById("q").addEventListener("keydown", (ev) => { if (ev.key === "Enter") search(); });
  const goLL = () => {
    const m = document.getElementById("ll").value.match(/(-?\d+(?:\.\d+)?)[,\s]+(-?\d+(?:\.\d+)?)/);
    if (m) setPoint(parseFloat(m[1]), parseFloat(m[2]), { pan: true });
  };
  const goXZ = () => {
    const m = document.getElementById("xz").value.match(/(-?\d+(?:\.\d+)?)[,\s]+(?:-?\d+(?:\.\d+)?[,\s]+)?(-?\d+(?:\.\d+)?)/);
    if (!m) return;
    const nums = document.getElementById("xz").value.match(/-?\d+(?:\.\d+)?/g).map(Number);
    // accept "X Z" or "X Y Z" (as copied from F3)
    const [x, z] = nums.length >= 3 ? [nums[0], nums[2]] : [nums[0], nums[1]];
    const [lat, lon] = JPProj.mcToLl(x + 0.5, z + 0.5);
    setPoint(lat, lon, { pan: true });
  };
  document.getElementById("llBtn").addEventListener("click", goLL);
  document.getElementById("ll").addEventListener("keydown", (ev) => { if (ev.key === "Enter") goLL(); });
  document.getElementById("xzBtn").addEventListener("click", goXZ);
  document.getElementById("xz").addEventListener("keydown", (ev) => { if (ev.key === "Enter") goXZ(); });
  document.getElementById("hsel").addEventListener("change", cmdText);
  document.querySelectorAll("button.copy").forEach((b) => b.addEventListener("click", () => copyText(document.getElementById(b.dataset.for).textContent, b)));
  document.addEventListener("langchange", () => { renderPlaces(); cmdText(); });
  // ?lat=..&lon=.. or the summit of Mt. Fuji
  const u = new URLSearchParams(location.search);
  if (u.get("lat") && u.get("lon")) setPoint(parseFloat(u.get("lat")), parseFloat(u.get("lon")), { pan: true });
  else {
    const fuji = places.find((p) => p.id === "fuji_summit");
    if (fuji) setPoint(fuji.lat, fuji.lon, { y: fuji.y });
  }
}

main();
