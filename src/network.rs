//! Global pass over OSM roads and railways: the vertical alignment of every
//! way that does not simply lie on the ground — bridges and viaducts, elevated
//! embankments, tunnels and subways — solved over the whole network at once,
//! so a viaduct or a subway that OSM splits into many ways at stations,
//! junctions and crossovers gets one consistent, smooth profile (junction
//! nodes are shared, never pinned to the ground).
//!
//! Mode of a way, from its tags:
//!   tunnel — tunnel=* (not building_passage / culvert / avalanche_protector),
//!            location=underground, level < 0, a subway with layer < 0
//!   bridge — bridge=*, man_made=bridge
//!   embank — embankment=yes with layer >= 1 that the DEM does not show
//!            (relief < 1 m: city viaducts of brick or earth): a solid fill
//!   ground — everything else (also indoor corridors); heights come from the
//!            DEM in the tile stage
//! A layer >= 1 (railways also level >= 1) without bridge tag, or a layered
//! embankment the DEM does show, only lifts a short piece (< 600 m, relief
//! < 2.5 m) between bridges or viaducts at both ends (elevated stations,
//! viaducts tagged piecewise): many railways at grade carry layer=1 for the
//! underpasses along them, and earth embankments are in the DEM already.
//! Steps leading onto a bridge or into a tunnel, and short ground pieces
//! between two structure ways of the same kind, join the structure.
//!
//! Heights (m, heightmap datum) on a 2 m node graph per connected component:
//!   anchors — nodes shared with a ground way of the same kind (portals,
//!     abutments; vehicle roads only by vehicle roads), bridge–tunnel
//!     junctions and dead ends of road bridges: the ground height there.
//!   target  — ground + clearance (bridges, by layer and kind) or ground −
//!     depth (tunnels, by layer), capped by what the grade allows from the
//!     anchors; smoothed (max/min filter then mean) along the network.
//!   base    — interpolation between the two nearest anchors: a short bridge
//!     never sags below, a tunnel never rises above the line between its ends.
//!   result  — max(base, target) for bridges, min for tunnels, then the grade
//!     limit along the network (dips of a viaduct over a valley, humps of a
//!     tunnel are flattened to the grade).
//!
//! Output `vec_net_struct.bin`: each structure way densified to ≤ 2 m with the
//! deck/floor surface height per vertex in `zs[0]`; class/nums/text from the
//! source way, flags with the bridge/tunnel/embankment bits replaced by the
//! mode (F_BRIDGE, F_TUNNEL or F_EMBANKMENT), nums[2] = effective layer,
//! nums[3] = 1 for rail, 0 for road.

use crate::heightmap::{K_HOLE, K_INLAND, K_LAND1, K_LAND5, K_SEA};
use crate::ingest::TILE_MARGIN;
use crate::osm::{F_BRIDGE, F_CUTTING, F_EMBANKMENT, F_HIGHSPEED, F_TUNNEL};
use crate::vfeat::{self, TiledReader, VFeat};
use crate::world::TileSet;
use anyhow::Result;
use rayon::prelude::*;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::Path;

fn key(p: [f64; 2]) -> (i64, i64) {
    ((p[0] * 1000.0).round() as i64, (p[1] * 1000.0).round() as i64)
}

fn tag<'a>(text: &'a str, k: &str) -> Option<&'a str> {
    text.split(';').find_map(|kv| kv.split_once('=').filter(|(kk, _)| *kk == k).map(|(_, v)| v))
}

/// First number of a tag value ("-1", "1;2", "0.5").
fn tag_num(text: &str, k: &str) -> Option<f32> {
    let v = tag(text, k)?;
    let s: String = v.trim().chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
    s.parse().ok()
}

fn load_all(path: &Path) -> Result<Vec<VFeat>> {
    let r = TiledReader::open(path)?;
    let mut seen = HashSet::new();
    let mut v = Vec::new();
    for (tx, tz) in r.tiles() {
        for f in r.tile(tx, tz) {
            if seen.insert(f.id) {
                v.push(f);
            }
        }
    }
    Ok(v)
}

fn len_of(p: &[[f64; 2]]) -> f64 {
    p.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum()
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Mode {
    Ground,
    Bridge,
    Embank,
    Tunnel,
}

impl Mode {
    fn up(self) -> bool {
        matches!(self, Mode::Bridge | Mode::Embank)
    }
}

/// Road classes that carry vehicles (osm::ROAD_CLASSES index + 1).
fn vehicle(class: u16) -> bool {
    matches!(class, 1..=16 | 23)
}

const STEPS: u16 = 21;
const CONSTRUCTION: u16 = 24;

/// Mode from the tags, the effective layer, and whether the elevation rests on
/// a layer/level tag alone (to be checked against the DEM).
fn mode_of(f: &VFeat, rail: bool) -> (Mode, f32, bool) {
    let t = f.text.as_str();
    let layer = tag_num(t, "layer").unwrap_or(0.0).round();
    let level = tag_num(t, "level");
    let tunnel = tag(t, "tunnel").is_some_and(|v| !matches!(v, "no" | "building_passage" | "culvert" | "avalanche_protector"))
        || tag(t, "location") == Some("underground");
    let bridge = tag(t, "bridge").is_some_and(|v| v != "no") || tag(t, "man_made") == Some("bridge");
    let embankment = tag(t, "embankment").is_some_and(|v| v != "no");
    // corridors inside buildings are not structures of their own
    if tag(t, "indoor").is_some_and(|v| v != "no") {
        return (Mode::Ground, 0.0, false);
    }
    if tunnel {
        return (Mode::Tunnel, layer.min(-1.0), false);
    }
    if bridge {
        return (Mode::Bridge, layer.max(1.0), false);
    }
    // a level tag alone lifts railways (elevated stations); for paths it is
    // mostly the floor of a building they run through
    let lv = if layer != 0.0 || !rail { layer } else { level.unwrap_or(0.0).round() };
    if lv >= 1.0 {
        return (if embankment { Mode::Embank } else { Mode::Bridge }, lv, true);
    }
    if level.is_some_and(|l| l <= -1.0) || (rail && layer <= -1.0 && tag(t, "railway") == Some("subway")) {
        return (Mode::Tunnel, lv.min(-1.0), false);
    }
    (Mode::Ground, 0.0, false)
}

struct Way {
    f: VFeat,
    rail: bool,
    mode: Mode,
    layer: f32,
    /// steps (or short ground pieces) that only lead onto / into a structure
    ramp: bool,
}

impl Way {
    /// Maximum grade along the way.
    fn grade(&self) -> f64 {
        if self.ramp && self.f.class == STEPS && !self.rail {
            return 0.8;
        }
        let t = self.f.text.as_str();
        if self.rail {
            if self.f.flags & F_HIGHSPEED != 0 {
                return 0.025;
            }
            return match tag(t, "railway") {
                Some("funicular") => 0.6,
                Some("subway" | "light_rail" | "monorail" | "tram") => 0.045,
                Some("narrow_gauge") => 0.04,
                _ => 0.035,
            };
        }
        match self.f.class {
            1 => 0.05,
            2 | 4 | 6 | 8 | 10 => 0.08,
            3 | 5 | 7 | 9 => 0.07,
            11..=16 | 23 => 0.1,
            STEPS => 0.8,
            _ => 0.12,
        }
    }

    /// Clearance (bridge, m above ground) or depth (tunnel, m below ground)
    /// of the deck / floor away from the ends.
    fn offset(&self) -> f64 {
        if self.ramp {
            return 0.0;
        }
        let l = (self.layer.abs().max(1.0) - 1.0) as f64;
        let t = self.f.text.as_str();
        if self.mode.up() {
            if self.rail {
                let b = if self.f.flags & F_HIGHSPEED != 0 {
                    10.0
                } else if matches!(tag(t, "railway"), Some("monorail" | "light_rail")) {
                    9.0
                } else {
                    7.5
                };
                b + 6.5 * l
            } else {
                let b = match self.f.class {
                    1..=4 => 7.0,
                    5..=16 | 23 => 6.0,
                    _ => 5.0,
                };
                b + 6.0 * l
            }
        } else if self.rail {
            10.0 + 7.0 * l
        } else if vehicle(self.f.class) {
            9.0 + 7.0 * l
        } else {
            6.0 + 5.0 * l
        }
    }
}

/// Ground height for structures at a point: mean of the land columns in a
/// (2r+1)² window; over water the water surface (bridges) or a little below
/// the bed (tunnels); NaN outside the data.
fn ground_at(ts: &TileSet, x: f64, z: f64, r: i64, up: bool) -> f32 {
    let (cx, cz) = (x.floor() as i64, z.floor() as i64);
    let (mut s, mut n) = (0f64, 0u32);
    let (mut water, mut wn) = (0f64, 0u32);
    for dz in -r..=r {
        for dx in -r..=r {
            if let Ok(Some((h, k))) = ts.sample(cx + dx, cz + dz) {
                match k {
                    K_LAND1 | K_LAND5 | K_HOLE => {
                        s += h as f64;
                        n += 1;
                    }
                    K_INLAND => {
                        water += h as f64;
                        wn += 1;
                    }
                    K_SEA => wn += 1, // sea level 0
                    _ => {}
                }
            }
        }
    }
    if n > 0 {
        (s / n as f64) as f32
    } else if wn > 0 {
        let w = (water / wn as f64) as f32;
        if up { w } else { w - 8.0 }
    } else {
        f32::NAN
    }
}

/// Median of (centre − mean of both sides, 20 m beyond the way) along a way:
/// how far the DEM already shows it raised above its surroundings
/// (embankments; a slope across the way cancels out).
fn dem_relief(ts: &TileSet, p: &[[f64; 2]], half_width: f64) -> f32 {
    let mut v: Vec<f32> = Vec::new();
    let total = len_of(p);
    let n = ((total / 10.0).ceil() as usize).clamp(1, 200);
    let mut acc = 0.0;
    let mut k = 0;
    for w in p.windows(2) {
        let (a, b) = (w[0], w[1]);
        let l = (b[0] - a[0]).hypot(b[1] - a[1]);
        if l < 1e-6 {
            continue;
        }
        let (dx, dz) = ((b[0] - a[0]) / l, (b[1] - a[1]) / l);
        while k < n && (k as f64 + 0.5) * total / n as f64 <= acc + l {
            let t = (k as f64 + 0.5) * total / n as f64 - acc;
            let (x, z) = (a[0] + dx * t, a[1] + dz * t);
            let off = half_width + 20.0;
            let c = ground_at(ts, x, z, 1, true);
            let l1 = ground_at(ts, x - dz * off, z + dx * off, 2, true);
            let r1 = ground_at(ts, x + dz * off, z - dx * off, 2, true);
            if c.is_finite() && l1.is_finite() && r1.is_finite() {
                v.push(c - (l1 + r1) / 2.0);
            }
            k += 1;
        }
        acc += l;
    }
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// Node graph of one component (CSR adjacency).
struct Graph {
    start: Vec<u32>,
    adj: Vec<(u32, f32, f32)>, // (neighbour, length m, grade)
}

impl Graph {
    fn nb(&self, v: usize) -> &[(u32, f32, f32)] {
        &self.adj[self.start[v] as usize..self.start[v + 1] as usize]
    }
}

#[derive(PartialEq)]
struct Item(f64, u32, u32);
impl Eq for Item {}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Item {
    // min-heap on the cost
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.partial_cmp(&self.0).unwrap_or(Ordering::Equal).then(o.1.cmp(&self.1)).then(o.2.cmp(&self.2))
    }
}

/// The two nearest anchors (by distance along the network, distinct anchors).
fn two_nearest(g: &Graph, n: usize, anchors: &[(u32, f64)]) -> Vec<[(f64, u32); 2]> {
    let none = (f64::INFINITY, u32::MAX);
    let mut lab = vec![[none, none]; n];
    let mut heap = BinaryHeap::new();
    for (ai, &(v, _)) in anchors.iter().enumerate() {
        heap.push(Item(0.0, v, ai as u32));
    }
    while let Some(Item(d, v, s)) = heap.pop() {
        let l = &mut lab[v as usize];
        if l[1].1 != u32::MAX || l[0].1 == s {
            continue;
        }
        if l[0].1 == u32::MAX {
            l[0] = (d, s);
        } else {
            l[1] = (d, s);
        }
        for &(u, len, _) in g.nb(v as usize) {
            let lu = &lab[u as usize];
            if lu[1].1 == u32::MAX && lu[0].1 != s {
                heap.push(Item(d + len as f64, u, s));
            }
        }
    }
    lab
}

/// min over anchors of (sign·h_a + Σ len·grade): the highest (sign +1, as
/// +value) or lowest (sign −1, as −value) height the grade allows.
fn reach(g: &Graph, n: usize, anchors: &[(u32, f64)], sign: f64) -> Vec<f64> {
    let mut best = vec![f64::INFINITY; n];
    let mut heap = BinaryHeap::new();
    for &(v, h) in anchors {
        let c = sign * h;
        if c < best[v as usize] {
            best[v as usize] = c;
            heap.push(Item(c, v, 0));
        }
    }
    while let Some(Item(c, v, _)) = heap.pop() {
        if c > best[v as usize] {
            continue;
        }
        for &(u, len, gr) in g.nb(v as usize) {
            let cu = c + len as f64 * gr as f64;
            if cu < best[u as usize] {
                best[u as usize] = cu;
                heap.push(Item(cu, u, 0));
            }
        }
    }
    best.iter().map(|&c| if c.is_finite() { sign * c } else { sign * f64::INFINITY }).collect()
}

#[derive(Clone, Copy)]
enum Op {
    Max,
    Min,
    Mean,
}

/// Window filter along the network: nodes within `r` m (network distance)
/// whose mask is set.
fn filter(g: &Graph, vals: &[f64], mask: &[bool], r: f64, op: Op) -> Vec<f64> {
    let n = vals.len();
    let mut out = vals.to_vec();
    let mut seen: Vec<u32> = vec![u32::MAX; n];
    let mut stack: Vec<(u32, f64)> = Vec::new();
    for v in 0..n {
        if !mask[v] {
            continue;
        }
        let (mut acc, mut cnt) = (match op {
            Op::Max => f64::NEG_INFINITY,
            Op::Min => f64::INFINITY,
            Op::Mean => 0.0,
        }, 0usize);
        stack.clear();
        stack.push((v as u32, 0.0));
        seen[v] = v as u32;
        while let Some((u, d)) = stack.pop() {
            let x = vals[u as usize];
            if mask[u as usize] && x.is_finite() {
                match op {
                    Op::Max => acc = acc.max(x),
                    Op::Min => acc = acc.min(x),
                    Op::Mean => acc += x,
                }
                cnt += 1;
            }
            for &(w, len, _) in g.nb(u as usize) {
                let dw = d + len as f64;
                if dw <= r && seen[w as usize] != v as u32 {
                    seen[w as usize] = v as u32;
                    stack.push((w, dw));
                }
            }
        }
        if cnt > 0 {
            out[v] = if let Op::Mean = op { acc / cnt as f64 } else { acc };
        }
    }
    out
}

/// Enforce the grade along the network: for bridges raise every node to at
/// least h_v − grade·d of any other node (no dips), for tunnels lower it to at
/// most h_v + grade·d (no humps). Fixed nodes stay.
fn grade_limit(g: &Graph, h: &mut [f64], movable: &[bool], up: bool) {
    let s = if up { -1.0 } else { 1.0 }; // heap pops the highest (up) / lowest (down) first
    let mut heap = BinaryHeap::new();
    for v in 0..h.len() {
        if h[v].is_finite() {
            heap.push(Item(s * h[v], v as u32, 0));
        }
    }
    while let Some(Item(c, v, _)) = heap.pop() {
        if (s * c - h[v as usize]).abs() > 1e-9 {
            continue;
        }
        let hv = h[v as usize];
        for &(u, len, gr) in g.nb(v as usize) {
            let u = u as usize;
            if !movable[u] {
                continue;
            }
            let cand = if up { hv - len as f64 * gr as f64 } else { hv + len as f64 * gr as f64 };
            if !h[u].is_finite() || (up && cand > h[u] + 1e-9) || (!up && cand < h[u] - 1e-9) {
                h[u] = cand;
                heap.push(Item(s * cand, u as u32, 0));
            }
        }
    }
}

/// Densify keeping the original vertices; returns points and whether each is
/// an original vertex.
fn densify(pts: &[[f64; 2]], step: f64) -> (Vec<[f64; 2]>, Vec<bool>) {
    let mut out = vec![pts[0]];
    let mut orig = vec![true];
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let d = (b[0] - a[0]).hypot(b[1] - a[1]);
        let n = (d / step).ceil().max(1.0) as usize;
        for i in 1..=n {
            let t = i as f64 / n as f64;
            out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
            orig.push(i == n);
        }
    }
    (out, orig)
}

/// Heights of one connected component of structure ways.
fn solve(ts: &TileSet, ways: &[Way], comp: &[usize], ground_nodes: &HashMap<(bool, (i64, i64)), u8>, touch: &HashMap<(bool, (i64, i64)), u32>) -> Vec<(usize, Vec<[f64; 2]>, Vec<f32>)> {
    // ---- nodes
    let mut node_of: HashMap<(i64, i64), u32> = HashMap::new();
    let (mut xs, mut zs): (Vec<f64>, Vec<f64>) = (Vec::new(), Vec::new());
    let mut way_nodes: Vec<(usize, Vec<[f64; 2]>, Vec<u32>)> = Vec::new();
    let mut edges: Vec<(u32, u32, f32, f32)> = Vec::new();
    let mut flags: Vec<u8> = Vec::new(); // 1 up, 2 down
    let mut off: Vec<f64> = Vec::new(); // clearance (up) / depth (down), min over the ways at a node
    let mut vehicle_road: Vec<bool> = Vec::new();
    let mut orig_key: Vec<Option<(i64, i64)>> = Vec::new();
    let new_node = |p: [f64; 2], k: Option<(i64, i64)>, xs: &mut Vec<f64>, zs: &mut Vec<f64>, flags: &mut Vec<u8>, off: &mut Vec<f64>, vr: &mut Vec<bool>, ok: &mut Vec<Option<(i64, i64)>>| -> u32 {
        xs.push(p[0]);
        zs.push(p[1]);
        flags.push(0);
        off.push(f64::INFINITY);
        vr.push(false);
        ok.push(k);
        (xs.len() - 1) as u32
    };
    for &wi in comp {
        let w = &ways[wi];
        let (pts, orig) = densify(&w.f.parts[0], 2.0);
        let mut ids = Vec::with_capacity(pts.len());
        for (p, &o) in pts.iter().zip(&orig) {
            let id = if o {
                let k = key(*p);
                match node_of.get(&k) {
                    Some(&id) => id,
                    None => {
                        let id = new_node(*p, Some(k), &mut xs, &mut zs, &mut flags, &mut off, &mut vehicle_road, &mut orig_key);
                        node_of.insert(k, id);
                        id
                    }
                }
            } else {
                new_node(*p, None, &mut xs, &mut zs, &mut flags, &mut off, &mut vehicle_road, &mut orig_key)
            };
            flags[id as usize] |= if w.mode.up() { 1 } else { 2 };
            off[id as usize] = off[id as usize].min(w.offset());
            vehicle_road[id as usize] |= !w.rail && vehicle(w.f.class);
            ids.push(id);
        }
        let gr = w.grade() as f32;
        for k in 1..ids.len() {
            let l = (pts[k][0] - pts[k - 1][0]).hypot(pts[k][1] - pts[k - 1][1]) as f32;
            if ids[k] != ids[k - 1] {
                edges.push((ids[k - 1], ids[k], l.max(0.01), gr));
            }
        }
        way_nodes.push((wi, pts, ids));
    }
    let n = xs.len();
    let mut start = vec![0u32; n + 1];
    for &(a, b, _, _) in &edges {
        start[a as usize + 1] += 1;
        start[b as usize + 1] += 1;
    }
    for i in 1..=n {
        start[i] += start[i - 1];
    }
    let mut fill = start.clone();
    let mut adj = vec![(0u32, 0f32, 0f32); edges.len() * 2];
    for &(a, b, l, gr) in &edges {
        adj[fill[a as usize] as usize] = (b, l, gr);
        fill[a as usize] += 1;
        adj[fill[b as usize] as usize] = (a, l, gr);
        fill[b as usize] += 1;
    }
    let g = Graph { start, adj };
    let rail = ways[comp[0]].rail;
    // ---- ground under every node
    let mut gr: Vec<f64> = (0..n).map(|i| ground_at(ts, xs[i], zs[i], 1, flags[i] & 2 == 0) as f64).collect();
    // outside the data (beyond the area): the nearest known ground along the
    // network, however far (a NaN here would leak through min/max into the heights)
    let mut queue: std::collections::VecDeque<usize> = (0..n).filter(|&i| gr[i].is_finite()).collect();
    while let Some(i) = queue.pop_front() {
        for &(j, _, _) in g.nb(i) {
            let j = j as usize;
            if !gr[j].is_finite() {
                gr[j] = gr[i];
                queue.push_back(j);
            }
        }
    }
    for v in gr.iter_mut().filter(|v| !v.is_finite()) {
        *v = 0.0;
    }
    // ---- anchors
    let mut anchor = vec![f64::NAN; n];
    for i in 0..n {
        let Some(k) = orig_key[i] else { continue };
        let both = flags[i] == 3;
        let gn = ground_nodes.get(&(rail, k)).copied().unwrap_or(0);
        // vehicle roads are only held by vehicle roads (a footpath touching a
        // road bridge mid-span does not pull it down)
        let held = if vehicle_road[i] { gn & 2 != 0 } else { gn != 0 };
        // a road bridge that ends without touching anything meets the ground there
        let dead_end = !rail && flags[i] & 1 != 0 && touch.get(&(rail, k)).copied().unwrap_or(0) <= 1 && g.nb(i).len() <= 1;
        if both || held || dead_end {
            anchor[i] = gr[i];
        }
    }
    let anchors: Vec<(u32, f64)> = (0..n).filter(|&i| anchor[i].is_finite()).map(|i| (i as u32, anchor[i])).collect();
    let lab = two_nearest(&g, n, &anchors);
    let up_lim = reach(&g, n, &anchors, 1.0);
    let down_lim = reach(&g, n, &anchors, -1.0);
    let is_up: Vec<bool> = (0..n).map(|i| flags[i] == 1).collect();
    let is_down: Vec<bool> = (0..n).map(|i| flags[i] == 2).collect();
    // ---- targets
    let mut t = vec![0f64; n];
    for i in 0..n {
        t[i] = if anchor[i].is_finite() {
            anchor[i]
        } else if is_up[i] {
            (gr[i] + off[i]).min(up_lim[i])
        } else {
            (gr[i] - off[i]).max(down_lim[i])
        };
    }
    let up_mask: Vec<bool> = (0..n).map(|i| is_up[i] || anchor[i].is_finite()).collect();
    let down_mask: Vec<bool> = (0..n).map(|i| is_down[i] || anchor[i].is_finite()).collect();
    let tu = filter(&g, &filter(&g, &t, &up_mask, 20.0, Op::Max), &up_mask, 15.0, Op::Mean);
    let td = filter(&g, &filter(&g, &t, &down_mask, 20.0, Op::Min), &down_mask, 15.0, Op::Mean);
    // ---- combine with the line between the nearest anchors
    let mut h = vec![0f64; n];
    for i in 0..n {
        if anchor[i].is_finite() {
            h[i] = anchor[i];
            continue;
        }
        let [(d1, a1), (d2, a2)] = lab[i];
        let base = if a2 != u32::MAX {
            let (h1, h2) = (anchors[a1 as usize].1, anchors[a2 as usize].1);
            Some(((h1 * d2 + h2 * d1) / (d1 + d2), d1 + d2))
        } else if a1 != u32::MAX {
            Some((anchors[a1 as usize].1, f64::INFINITY))
        } else {
            None
        };
        if is_up[i] {
            // a bridge stays on the line between its abutments over a short
            // span (valleys); long viaducts follow the ground
            let b = base.filter(|b| b.1 < 2000.0).map(|b| b.0).unwrap_or(f64::NEG_INFINITY);
            h[i] = b.max(tu[i]).min(up_lim[i].max(b));
        } else {
            let b = base.map(|b| b.0).unwrap_or(f64::INFINITY);
            h[i] = b.min(td[i]).max(down_lim[i].min(b));
        }
    }
    let movable_up: Vec<bool> = (0..n).map(|i| is_up[i] && !anchor[i].is_finite()).collect();
    let movable_down: Vec<bool> = (0..n).map(|i| is_down[i] && !anchor[i].is_finite()).collect();
    grade_limit(&g, &mut h, &movable_up, true);
    grade_limit(&g, &mut h, &movable_down, false);
    way_nodes.into_iter().map(|(wi, pts, ids)| (wi, pts, ids.iter().map(|&i| h[i as usize] as f32).collect())).collect()
}

pub fn run(work: &Path, hm_dir: &Path) -> Result<()> {
    let t0 = std::time::Instant::now();
    let mut ways: Vec<Way> = Vec::new();
    for (file, rail) in [("vec_osm_road.bin", false), ("vec_osm_rail.bin", true)] {
        for f in load_all(&work.join(file))? {
            if f.parts.first().is_some_and(|p| p.len() >= 2) && !f.area && !(!rail && f.class == CONSTRUCTION) {
                let (mode, layer, _) = mode_of(&f, rail);
                ways.push(Way { f, rail, mode, layer, ramp: false });
            }
        }
    }
    eprintln!("network: {} ways in {:.1}s", ways.len(), t0.elapsed().as_secs_f64());
    let ts = TileSet::new(hm_dir);
    // Elevation resting on a layer tag alone, or a layered embankment, is only
    // believed where the terrain agrees: many railways at grade carry layer=1
    // for the underpasses along them, and earth embankments are in the DEM
    // already. Such a way is lifted when it is an embankment the DEM does not
    // show (city viaducts of brick or earth: relief < 1 m), or a short piece
    // (< 600 m, relief < 2.5 m) between bridges or viaducts at both ends.
    let cands: Vec<usize> = (0..ways.len()).filter(|&i| ways[i].mode.up() && mode_of(&ways[i].f, ways[i].rail).2).collect();
    let relief: Vec<f32> = cands
        .par_iter()
        .map(|&i| {
            let w = &ways[i];
            let hw = if w.rail { 3.0 } else { (crate::infra::road_width(w.f.class, w.f.nums, &w.f.text) / 2.0) as f64 };
            dem_relief(&ts, &w.f.parts[0], hw)
        })
        .collect();
    let mut pending: Vec<(usize, Mode, f32)> = Vec::new();
    let mut lifted = 0;
    for (k, &i) in cands.iter().enumerate() {
        let r = relief[k];
        let w = &mut ways[i];
        let (mode, layer) = (w.mode, w.layer);
        (w.mode, w.layer) = (Mode::Ground, 0.0);
        if mode == Mode::Embank && r < 1.0 {
            (w.mode, w.layer) = (mode, layer);
            lifted += 1;
        } else if r < 2.5 && len_of(&w.f.parts[0]) < 600.0 {
            pending.push((i, mode, layer));
        }
    }
    loop {
        let upv: HashSet<(bool, (i64, i64))> = ways.iter().filter(|w| w.mode.up()).flat_map(|w| w.f.parts[0].iter().map(move |q| (w.rail, key(*q)))).collect();
        let (now, wait): (Vec<_>, Vec<_>) = pending.into_iter().partition(|&(i, _, _)| {
            let w = &ways[i];
            let p = &w.f.parts[0];
            upv.contains(&(w.rail, key(p[0]))) && upv.contains(&(w.rail, key(*p.last().unwrap())))
        });
        pending = wait;
        if now.is_empty() {
            break;
        }
        for (i, mode, layer) in now {
            (ways[i].mode, ways[i].layer) = (mode, layer);
            lifted += 1;
        }
    }
    eprintln!("network: {} ways raised by a layer tag alone or layered embankments, {lifted} of them lifted ({:.1}s)", cands.len(), t0.elapsed().as_secs_f64());
    // number of ways through every vertex, by kind
    let mut touch: HashMap<(bool, (i64, i64)), u32> = HashMap::new();
    for w in &ways {
        for q in &w.f.parts[0] {
            *touch.entry((w.rail, key(*q))).or_default() += 1;
        }
    }
    // steps onto a bridge / into a tunnel, and short ground pieces between two
    // structure ways (a landing between a footbridge and its stairs, a gap in
    // the tagging of a viaduct), join the structure
    for _round in 0..3 {
        // structure ways through each vertex: (count, mode, layer; bridges preferred)
        let mut svert: HashMap<(bool, (i64, i64)), (u32, Mode, f32)> = HashMap::new();
        for w in ways.iter().filter(|w| w.mode != Mode::Ground) {
            for q in &w.f.parts[0] {
                let e = svert.entry((w.rail, key(*q))).or_insert((0, w.mode, w.layer));
                e.0 += 1;
                if w.mode.up() && !e.1.up() {
                    (e.1, e.2) = (w.mode, w.layer);
                }
            }
        }
        let mut changes: Vec<(usize, Mode, f32, bool)> = Vec::new();
        for (i, w) in ways.iter().enumerate() {
            if w.mode != Mode::Ground {
                continue;
            }
            let p = &w.f.parts[0];
            let mut side: [Option<(Mode, f32)>; 2] = [None, None];
            let mut other_ground = [false; 2];
            for (e, q) in [p[0], *p.last().unwrap()].into_iter().enumerate() {
                let k = (w.rail, key(q));
                let s = svert.get(&k).copied();
                let all = touch.get(&k).copied().unwrap_or(1);
                other_ground[e] = all > 1 + s.map(|s| s.0).unwrap_or(0);
                side[e] = s.map(|s| (s.1, s.2));
            }
            let steps = !w.rail && w.f.class == STEPS;
            if steps {
                // stairs: one end on the structure is enough
                if let Some((m, l)) = side[0].or(side[1]) {
                    changes.push((i, m, l, true));
                }
            } else if let (Some((m0, l0)), Some((m1, _))) = (side[0], side[1]) {
                if m0.up() == m1.up() && !other_ground[0] && !other_ground[1] && len_of(p) < 60.0 {
                    changes.push((i, m0, l0, false));
                }
            }
        }
        if changes.is_empty() {
            break;
        }
        for (i, m, l, ramp) in changes {
            let w = &mut ways[i];
            w.mode = if m == Mode::Embank && ramp { Mode::Bridge } else { m };
            w.layer = l;
            w.ramp = ramp;
        }
    }
    let mut counts: HashMap<(bool, Mode), (usize, f64)> = HashMap::new();
    for w in &ways {
        let e = counts.entry((w.rail, w.mode)).or_default();
        e.0 += 1;
        e.1 += len_of(&w.f.parts[0]) / 1000.0;
    }
    let mut cv: Vec<_> = counts.into_iter().collect();
    cv.sort_by_key(|c| (c.0.0, c.0.1 as u8));
    for ((rail, m), (c, km)) in cv {
        eprintln!("network: {} {:?}: {c} ways, {km:.0} km", if rail { "rail" } else { "road" }, m);
    }
    // ground nodes: bit 1 any ground way, bit 2 a vehicle road
    let mut ground_nodes: HashMap<(bool, (i64, i64)), u8> = HashMap::new();
    for w in &ways {
        if w.mode != Mode::Ground {
            continue;
        }
        let bit = if !w.rail && vehicle(w.f.class) { 3 } else { 1 };
        for q in &w.f.parts[0] {
            *ground_nodes.entry((w.rail, key(*q))).or_default() |= bit;
        }
    }
    // connected components of structure ways (shared vertices, same kind)
    let sids: Vec<usize> = (0..ways.len()).filter(|&i| ways[i].mode != Mode::Ground).collect();
    let mut parent: Vec<usize> = (0..ways.len()).collect();
    fn find(p: &mut Vec<usize>, mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    let mut first: HashMap<(bool, (i64, i64)), usize> = HashMap::new();
    for &i in &sids {
        for q in &ways[i].f.parts[0] {
            let k = (ways[i].rail, key(*q));
            match first.get(&k) {
                Some(&j) => {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    if a != b {
                        parent[a] = b;
                    }
                }
                None => {
                    first.insert(k, i);
                }
            }
        }
    }
    let mut comps: HashMap<usize, Vec<usize>> = HashMap::new();
    for &i in &sids {
        let r = find(&mut parent, i);
        comps.entry(r).or_default().push(i);
    }
    let mut comps: Vec<Vec<usize>> = comps.into_values().collect();
    for c in comps.iter_mut() {
        c.sort();
    }
    // spatially sorted so the heightmap block cache stays warm; big ones first
    comps.sort_by_key(|c| {
        let p = ways[c[0]].f.parts[0][0];
        ((p[1] / 10240.0).floor() as i64, (p[0] / 10240.0).floor() as i64, c[0])
    });
    let largest = comps.iter().map(|c| c.len()).max().unwrap_or(0);
    eprintln!("network: {} structure ways in {} components (largest {largest} ways) ({:.1}s)", sids.len(), comps.len(), t0.elapsed().as_secs_f64());
    let out: Vec<Vec<VFeat>> = comps
        .par_iter()
        .map(|comp| {
            solve(&ts, &ways, comp, &ground_nodes, &touch)
                .into_iter()
                .map(|(wi, p, z)| {
                    let w = &ways[wi];
                    let src = &w.f;
                    let mut nums = src.nums;
                    nums[2] = w.layer;
                    nums[3] = if w.rail { 1.0 } else { 0.0 };
                    let mode_bit = match w.mode {
                        Mode::Bridge => F_BRIDGE,
                        Mode::Embank => F_EMBANKMENT,
                        _ => F_TUNNEL,
                    };
                    let flags = (src.flags & !(F_BRIDGE | F_TUNNEL | F_EMBANKMENT | F_CUTTING)) | mode_bit;
                    VFeat { class: src.class, flags, nums, id: src.id, text: src.text.clone(), area: false, parts: vec![p], zs: vec![z] }
                })
                .collect()
        })
        .collect();
    let mut feats: Vec<VFeat> = out.into_iter().flatten().collect();
    feats.sort_by_key(|f| (f.nums[3] as u8, f.id));
    vfeat::write_tiled(&work.join("vec_net_struct.bin"), &feats, TILE_MARGIN)?;
    eprintln!("network: {} structure ways profiled in {:.1}s", feats.len(), t0.elapsed().as_secs_f64());
    Ok(())
}
