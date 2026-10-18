//! Audit of the generated roads, bridges, tunnels, buildings and trees: runs
//! the region generator (no light, no files written) for a set of regions and
//! counts what should not be there, with example places for each kind:
//!   - blocks in the clearance above a road surface (tree leaves and trunks,
//!     building walls, the underside of a deck, a tunnel ceiling),
//!   - road pixels of the feature layer that were not drawn (a building there),
//!   - steps between neighbouring road surfaces (junctions, bridge ends,
//!     tunnel portals) and between neighbouring rail blocks,
//!   - ground roads far above or below the terrain they lie on.
//! `AUDIT_TRACE=<way id>` prints the surface walked along one road (stderr).

use crate::chunk::{Extra, FeatSet, HALO, RW};
use crate::infra::{S_DECK, S_EDGE, S_EMBANK, S_GROUND, S_PORTAL, S_RAIL, S_TUNNEL, S_TUNNEL_WALL};
use crate::rgen::{CF_ROAD, Col, Ids, Overlay};
use crate::world::TileSet;
use anyhow::Result;
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

const KIND_NAMES: [&str; 11] = ["", "ground", "deck", "edge", "embank", "tunnel", "portal", "twall", "rail", "", ""];

#[derive(Default)]
struct Stats {
    regions: usize,
    /// walkable road surfaces by kind (core columns)
    surfaces: BTreeMap<&'static str, u64>,
    /// counters by category
    count: BTreeMap<String, u64>,
    /// examples: category -> (x, y, z, detail)
    examples: BTreeMap<String, Vec<(i64, i32, i64, String)>>,
}

impl Stats {
    fn add(&mut self, cat: &str, n: u64) {
        *self.count.entry(cat.to_string()).or_default() += n;
    }
    fn merge(&mut self, o: Stats) {
        self.regions += o.regions;
        for (k, v) in o.surfaces {
            *self.surfaces.entry(k).or_default() += v;
        }
        for (k, v) in o.count {
            *self.count.entry(k).or_default() += v;
        }
        for (k, v) in o.examples {
            self.examples.entry(k).or_default().extend(v);
        }
    }
}

/// Examples per region and category: the place of the first event in each of
/// a few chunks (spread over the region).
struct Ex {
    by_chunk: BTreeMap<String, BTreeMap<(i64, i64), (u32, i64, i32, i64, String)>>,
}

impl Ex {
    fn new() -> Self {
        Ex { by_chunk: BTreeMap::new() }
    }
    fn note(&mut self, cat: &str, x: i64, y: i32, z: i64, detail: impl FnOnce() -> String) {
        let m = self.by_chunk.entry(cat.to_string()).or_default();
        let e = m.entry((x.div_euclid(16), z.div_euclid(16))).or_insert_with(|| (0, x, y, z, detail()));
        e.0 += 1;
    }
    /// the chunks with the most events (ties: chunk order), `k` per category
    fn take(self, k: usize, st: &mut Stats) {
        for (cat, m) in self.by_chunk {
            let mut v: Vec<(u32, i64, i32, i64, String)> = m.into_values().collect();
            v.sort_by(|a, b| b.0.cmp(&a.0).then((a.1, a.3).cmp(&(b.1, b.3))));
            let out = st.examples.entry(cat).or_default();
            for (c, x, y, z, d) in v.into_iter().take(k) {
                out.push((x, y, z, format!("{d} (x{c} in chunk)")));
            }
        }
    }
}

fn block_at(cols: &[Col], ov: &Overlay, i: usize, y: i32, ids: &Ids) -> u16 {
    match ov.v[i].binary_search_by_key(&y, |e| e.0) {
        Ok(k) => ov.v[i][k].1,
        Err(_) => cols[i].block(y, ids),
    }
}

fn audit_region(ts: &TileSet, fs: &FeatSet, structs: &crate::vfeat::TiledReader, roads: &[crate::vfeat::VFeat], st: &crate::rgen::SpecTable, rx: i64, rz: i64) -> Result<Option<Stats>> {
    let Some(inp) = ts.region_input(rx, rz)? else { return Ok(None) };
    let per = crate::heightmap::TILE / 512;
    let (x0, z0, x1, z1) = (inp.ox as f64, inp.oz as f64, (inp.ox + RW as i64) as f64, (inp.oz + RW as i64) as f64);
    let tile_structs: Vec<crate::vfeat::VFeat> = structs
        .tile(rx.div_euclid(per), rz.div_euclid(per))
        .into_iter()
        .filter(|f| {
            let (a, b, c, d) = f.bbox();
            c >= x0 - 20.0 && a <= x1 + 20.0 && d >= z0 - 20.0 && b <= z1 + 20.0
        })
        .collect();
    let ex = Extra { fw: fs.window(inp.ox, inp.oz), st, structs: tile_structs };
    // nothing to check without roads or rails
    if !ex.fw.road.iter().any(|&r| r != 0) && !ex.fw.rail.iter().any(|&r| r != 0) && ex.structs.is_empty() {
        return Ok(None);
    }
    let (cols, ov, decks) = crate::chunk::generate(&inp, &ex);
    let ids = Ids::get();
    let defs = crate::blocks::Registry::get().snapshot();
    let n = RW;
    let core = |i: usize| -> bool {
        let (x, z) = (i % n, i / n);
        (HALO..HALO + 512).contains(&x) && (HALO..HALO + 512).contains(&z)
    };
    let wx = |i: usize| inp.ox + (i % n) as i64;
    let wz = |i: usize| inp.oz + (i / n) as i64;
    let mut stats = Stats { regions: 1, ..Default::default() };
    let mut exs = Ex::new();
    // surfaces per column (road kinds and rails)
    let mut by_col: Vec<Vec<(i32, u8, u8)>> = vec![Vec::new(); n * n];
    for &(i, y2, kind, code) in &decks.surf {
        let c = &cols[i as usize];
        // skip ground surfaces replaced later (platforms, bridge ends, tunnel mouths)
        if kind == S_GROUND && y2 != 2 * c.g + 2 && y2 != 2 * c.g + 3 {
            continue;
        }
        by_col[i as usize].push((y2, kind, code));
    }
    let walkable = |k: u8| matches!(k, S_GROUND | S_DECK | S_EMBANK | S_TUNNEL | S_PORTAL);
    let fx = &ex.fw;
    for i in 0..n * n {
        if !core(i) {
            continue;
        }
        for &(y2, kind, code) in &by_col[i] {
            if kind == S_RAIL {
                *stats.surfaces.entry("rail").or_default() += 1;
            }
            if !walkable(kind) {
                continue;
            }
            // a ground road surface later replaced (a platform, a bridge end, a tunnel mouth) is not there
            if kind == S_GROUND && y2 != 2 * cols[i].g + 2 && y2 != 2 * cols[i].g + 3 {
                stats.add("ground_superseded", 1);
                continue;
            }
            *stats.surfaces.entry(KIND_NAMES[kind as usize]).or_default() += 1;
            // the block the surface stands on must still be there
            // (a bottom slab for an odd 2·Y, else the full block under the surface)
            let below = if y2.rem_euclid(2) == 1 { (y2 - 1).div_euclid(2) } else { y2.div_euclid(2) - 1 };
            let bb = block_at(&cols, &ov, i, below, ids);
            let bname = defs[bb as usize].name.trim_start_matches("minecraft:");
            // a street tree stands on the sidewalk, the median or a plaza (its
            // trunk replaces the half slab there): counted, not a fault
            let walk = matches!(code, crate::infra::RD_SIDEWALK | crate::infra::RD_PEDESTRIAN | crate::infra::RD_MEDIAN);
            let tree_block = |s: &str| s.ends_with("_leaves") || s.ends_with("_log") || s == "bamboo";
            if walk && tree_block(bname) {
                stats.add("street_tree_on_walk", 1);
                continue;
            }
            if bb == ids.air || bname.ends_with("_leaves") || bname.ends_with("_log") || bname == "water" {
                stats.add("surface_replaced", 1);
                exs.note("surface_replaced", wx(i), below, wz(i), || format!("{} under a {} surface", bname, KIND_NAMES[kind as usize]));
            }
            // clearance: the 4 blocks above the walking surface
            let yf = (y2 + 1).div_euclid(2);
            let mut worst: Option<(&str, i32, String)> = None;
            for dy in 0..4 {
                let y = yf + dy;
                let bk = block_at(&cols, &ov, i, y, ids);
                if bk == ids.air {
                    continue;
                }
                let name = defs[bk as usize].name.trim_start_matches("minecraft:");
                if name == "rail" || (name == "azalea_leaves" && code == crate::infra::RD_MEDIAN) || (dy == 0 && kind == S_GROUND && name.ends_with("_slab")) {
                    continue;
                }
                if walk && tree_block(name) {
                    stats.add("street_tree_on_walk", 1);
                    break;
                }
                let cat = if tree_block(name) {
                    "tree"
                } else if fx.bldg[i] != 0 {
                    "building"
                } else if decks.bottom[i] <= y && kind != S_TUNNEL {
                    "deck_above"
                } else if kind == S_TUNNEL {
                    "tunnel_ceiling"
                } else {
                    "other"
                };
                worst = Some((cat, dy, name.to_string()));
                break;
            }
            if let Some((cat, dy, name)) = worst {
                let low = if dy < 2 { "low" } else { "high" };
                let key = format!("clear_{cat}_{low}_{}", KIND_NAMES[kind as usize]);
                stats.add(&key, 1);
                exs.note(&key, wx(i), yf + dy, wz(i), || {
                    let b = &fx.bld[fx.bldg[i] as usize];
                    let bi = if fx.bldg[i] != 0 { format!(" [bldg src {} usage {} roof {} base {:.1} h {:.1}]", b.src, b.usage, b.roof, b.base, b.height) } else { String::new() };
                    format!("{name} {dy} above a {} road (code {code}){bi}", KIND_NAMES[kind as usize])
                });
                if cat == "other" {
                    stats.add(&format!("other_block:{name}"), 1);
                }
            }
            // a ground road far from the terrain it lies on
            if kind == S_GROUND {
                let dem = crate::rgen::SEA_Y as f32 + inp.h[i] + 1.0;
                let d = y2 as f32 / 2.0 - dem;
                if inp.k[i] != crate::heightmap::K_SEA && inp.k[i] != crate::heightmap::K_INLAND && d.abs() >= 3.0 {
                    let key = if d > 0.0 { "ground_road_above_dem" } else { "ground_road_below_dem" };
                    stats.add(key, 1);
                    exs.note(key, wx(i), yf, wz(i), || format!("road {:.1} m {} the DEM (code {code})", d.abs(), if d > 0.0 { "above" } else { "below" }));
                }
            }
            // a tunnel needs room
            if kind == S_TUNNEL {
                let mut free = 0;
                while free < 8 && block_at(&cols, &ov, i, yf + free, ids) == ids.air {
                    free += 1;
                }
                if free < 3 {
                    stats.add("tunnel_headroom_lt3", 1);
                    exs.note("tunnel_headroom_lt3", wx(i), yf, wz(i), || format!("{free} blocks of headroom (code {code})"));
                }
            }
        }
        // road pixels of the layer that were not drawn
        if fx.road[i] != 0 && fx.roadh[i].is_finite() && !cols[i].out && cols[i].flags & CF_ROAD == 0 {
            let vehicle = matches!(fx.road[i], 1..=5 | 11 | 12);
            let why = if fx.bldg[i] != 0 { "building" } else { "other" };
            let key = format!("road_px_missing_{why}_{}", if vehicle { "vehicle" } else { "foot" });
            stats.add(&key, 1);
            exs.note(&key, wx(i), cols[i].g + 1, wz(i), || format!("road code {} not drawn ({why})", fx.road[i]));
        }
    }
    // steps between neighbouring surfaces (each edge once: right and down neighbours)
    for i in 0..n * n {
        if !core(i) || by_col[i].is_empty() {
            continue;
        }
        let (x, z) = (i % n, i / n);
        for j in [if x + 1 < n { Some(i + 1) } else { None }, if z + 1 < n { Some(i + n) } else { None }].into_iter().flatten() {
            for &(a, ka, ca) in &by_col[i] {
                let rail_a = ka == S_RAIL;
                if !(walkable(ka) || rail_a) {
                    continue;
                }
                // the nearest surface of the same family next door
                let best = by_col[j].iter().filter(|s| if rail_a { s.1 == S_RAIL } else { walkable(s.1) }).min_by_key(|s| (s.0 - a).abs());
                let Some(&(b, kb, cb)) = best else { continue };
                let d = (a - b).abs(); // half blocks
                if rail_a {
                    if (4..=12).contains(&d) {
                        stats.add("rail_step_ge2", 1);
                        exs.note("rail_step_ge2", wx(i), a.div_euclid(2), wz(i), || format!("rails {} blocks apart", d as f32 / 2.0));
                    }
                    continue;
                }
                if !(2..=12).contains(&d) {
                    continue;
                }
                let (k1, k2) = if ka <= kb { (ka, kb) } else { (kb, ka) };
                let bucket = match d {
                    2 => "1",
                    3..=4 => "1.5-2",
                    5..=8 => "2.5-4",
                    _ => "4.5-6",
                };
                let key = format!("step_{}-{}_{bucket}", KIND_NAMES[k1 as usize], KIND_NAMES[k2 as usize]);
                stats.add(&key, 1);
                exs.note(&key, wx(i), a.div_euclid(2), wz(i), || {
                    format!("{} m step: {} (code {ca}) / {} (code {cb}) {}", d as f32 / 2.0, KIND_NAMES[ka as usize], KIND_NAMES[kb as usize], if j == i + 1 { "east" } else { "south" })
                });
            }
        }
    }
    // walk along the centre line of every vehicle road: steps in the drawn
    // surface under it (at junctions, where a way is split, at bridge ends
    // and tunnel portals, or along the way)
    let profiles: std::collections::HashMap<u64, (&Vec<[f64; 2]>, &Vec<f32>)> = ex
        .structs
        .iter()
        .filter(|f| f.nums[3] < 0.5)
        .filter_map(|f| Some((f.id, (f.parts.first()?, f.zs.first()?))))
        .collect();
    let trace: Option<u64> = std::env::var("AUDIT_TRACE").ok().and_then(|s| s.parse().ok());
    let mut deg: std::collections::HashMap<(i64, i64), (u32, bool)> = std::collections::HashMap::new();
    let key = |p: [f64; 2]| ((p[0] * 1000.0).round() as i64, (p[1] * 1000.0).round() as i64);
    for f in roads.iter().filter(|f| !f.area && !f.parts.is_empty()) {
        let structure = profiles.contains_key(&f.id);
        for (k, q) in f.parts[0].iter().enumerate() {
            let e = deg.entry(key(*q)).or_insert((0, false));
            e.0 += 1;
            // the ends of a bridge or tunnel
            e.1 |= structure && (k == 0 || k + 1 == f.parts[0].len());
        }
    }
    for f in roads.iter().filter(|f| !f.area && !f.parts.is_empty() && matches!(f.class, 1..=16 | 23)) {
        let (lo, hi) = (inp.ox as f64 + HALO as f64, inp.ox as f64 + (HALO + 512) as f64);
        let (lz, hz) = (inp.oz as f64 + HALO as f64, inp.oz as f64 + (HALO + 512) as f64);
        let (a, b, c, d) = f.bbox();
        if c < lo || a > hi || d < lz || b > hz {
            continue;
        }
        let pts = &f.parts[0];
        // arc positions of the special vertices of this way
        let mut arc = 0.0;
        let mut marks: Vec<(f64, u8)> = Vec::new(); // 1 junction / split, 2 structure end
        for k in 0..pts.len() {
            if k > 0 {
                arc += (pts[k][0] - pts[k - 1][0]).hypot(pts[k][1] - pts[k - 1][1]);
            }
            if let Some(&(n, send)) = deg.get(&key(pts[k])) {
                if send {
                    marks.push((arc, 2));
                } else if n >= 2 {
                    marks.push((arc, 1));
                }
            }
        }
        let prof = profiles.get(&f.id);
        let (dp, ds) = crate::roads::densify(pts, 1.0);
        let mut prev: Option<(i32, u8)> = None;
        for (q, &s) in dp.iter().zip(&ds) {
            let (px, pz) = ((q[0] - inp.ox as f64).floor() as i64, (q[1] - inp.oz as f64).floor() as i64);
            if px < 0 || pz < 0 || px >= n as i64 || pz >= n as i64 {
                prev = None;
                continue;
            }
            let i = pz as usize * n + px as usize;
            // expected: the last surface, else the profile of a structure or the terrain
            let want = match prev {
                Some((y, _)) => y,
                None => match prof {
                    Some((pp, zz)) => {
                        let k = pp.iter().enumerate().min_by(|a, b| ((a.1[0] - q[0]).hypot(a.1[1] - q[1])).total_cmp(&(b.1[0] - q[0]).hypot(b.1[1] - q[1]))).map(|(k, _)| k).unwrap_or(0);
                        2 * (crate::rgen::SEA_Y + zz[k].round() as i32 + 1)
                    }
                    None => 2 * (crate::rgen::SEA_Y + inp.h[i].round() as i32 + 1),
                },
            };
            let Some(&(y2, kind, _)) = by_col[i].iter().filter(|s| walkable(s.1)).min_by_key(|s| (s.0 - want).abs()) else {
                prev = None;
                continue;
            };
            if let Some((py2, pk)) = prev {
                let dd = (y2 - py2).abs();
                if dd >= 2 && core(i) {
                    let near = marks.iter().filter(|m| (m.0 - s).abs() <= 6.0).map(|m| m.1).max().unwrap_or(0);
                    let place = match near {
                        2 => "structure_end",
                        1 => "junction",
                        _ => "along",
                    };
                    let (k1, k2) = if pk <= kind { (pk, kind) } else { (kind, pk) };
                    let bucket = match dd {
                        2 => "1",
                        3..=4 => "1.5-2",
                        5..=8 => "2.5-4",
                        _ => "4.5+",
                    };
                    let key = format!("walk_{place}_{}-{}_{bucket}", KIND_NAMES[k1 as usize], KIND_NAMES[k2 as usize]);
                    stats.add(&key, 1);
                    exs.note(&key, wx(i), y2.div_euclid(2), wz(i), || format!("{} m step along way {} (class {})", dd as f32 / 2.0, f.id, f.class));
                }
            }
            if core(i) {
                stats.add("walk_samples", 1);
            }
            if trace == Some(f.id) {
                eprintln!("trace {} s={s:.0} x={} z={} surface={:.1} kind={} dem={:.1} n_surf={}", f.id, wx(i), wz(i), y2 as f32 / 2.0, KIND_NAMES[kind as usize], crate::rgen::SEA_Y as f32 + inp.h[i] + 1.0, by_col[i].len());
            }
            prev = Some((y2, kind));
        }
    }
    let _ = (S_EDGE, S_TUNNEL_WALL);
    exs.take(4, &mut stats);
    Ok(Some(stats))
}

/// Audit `regions`, print the summary and write the examples to `out` (TSV).
pub fn run(hm_dir: &Path, feat_dir: &Path, work: &Path, st: &crate::rgen::SpecTable, regions: &[(i64, i64)], out: &Path) -> Result<()> {
    let t0 = std::time::Instant::now();
    let ts = TileSet::new(hm_dir);
    let fs = FeatSet::new(feat_dir);
    let structs = crate::vfeat::TiledReader::open(&work.join("vec_net_struct.bin"))?;
    let road_rd = crate::vfeat::TiledReader::open(&work.join("vec_osm_road.bin"))?;
    // tile by tile, so the readers' caches stay warm
    let per = crate::heightmap::TILE / 512;
    let mut by_tile: BTreeMap<(i64, i64), Vec<(i64, i64)>> = BTreeMap::new();
    for &(rx, rz) in regions {
        by_tile.entry((rz.div_euclid(per), rx.div_euclid(per))).or_default().push((rx, rz));
    }
    let mut total = Stats::default();
    let done = std::sync::atomic::AtomicUsize::new(0);
    for ((tz, tx), rs) in by_tile {
        let roads = road_rd.tile(tx, tz);
        let parts: Vec<Stats> = rs
            .par_iter()
            .filter_map(|&(rx, rz)| {
                let r = audit_region(&ts, &fs, &structs, &roads, st, rx, rz);
                let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if d % 200 == 0 {
                    eprintln!("  audit: {d}/{} regions, {:.0}s", regions.len(), t0.elapsed().as_secs_f64());
                }
                r.ok().flatten()
            })
            .collect();
        for p in parts {
            total.merge(p);
        }
    }
    println!("audit: {} regions with roads or rails ({} asked) in {:.0}s", total.regions, regions.len(), t0.elapsed().as_secs_f64());
    println!("walkable road surfaces (columns):");
    for (k, v) in &total.surfaces {
        println!("  {k:10} {v:12}");
    }
    let surf_all: u64 = total.surfaces.values().sum();
    println!("findings (count; per 10,000 road columns):");
    for (k, v) in &total.count {
        if k.starts_with("other_block:") {
            continue;
        }
        println!("  {k:44} {v:10} {:10.2}", *v as f64 * 1e4 / surf_all.max(1) as f64);
    }
    let mut other: Vec<(&String, &u64)> = total.count.iter().filter(|(k, _)| k.starts_with("other_block:")).collect();
    other.sort_by(|a, b| b.1.cmp(a.1));
    if !other.is_empty() {
        println!("other blocks over roads: {}", other.iter().take(12).map(|(k, v)| format!("{} {v}", k.trim_start_matches("other_block:"))).collect::<Vec<_>>().join(", "));
    }
    let mut f = std::io::BufWriter::new(std::fs::File::create(out)?);
    writeln!(f, "category\tx\ty\tz\tdetail\ttp")?;
    for (cat, v) in &total.examples {
        for (x, y, z, d) in v {
            writeln!(f, "{cat}\t{x}\t{y}\t{z}\t{d}\t/tp @s {x} {} {z}", y + 2)?;
        }
    }
    eprintln!("audit: examples -> {}", out.display());
    Ok(())
}
