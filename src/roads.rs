//! Ground-level roads, railways and platforms for the feature rasters.
//!
//! Roads: OSM centerlines (bridges and tunnels are drawn from the network
//! profiles instead, see network.rs / infra.rs). The real width comes from
//! 基盤地図情報 道路縁 (RdEdg) where a ray cast across the road from the
//! centerline finds one, sidewalks from 道路構成線 歩道 (RdCompt). The surface
//! height is the cross-section median of the DEM, smoothed along the road, so
//! roads are flat across and grade evenly along. Lane markings follow the road
//! class. Everything is computed from the way geometry alone (densified from
//! its start point), so neighbouring tiles agree along their seams.
//! Rails: OSM tracks (`tracks=N` fanned out), 4-connected so rail blocks link
//! up, with ballast (or slab track for the Shinkansen). Platforms take the
//! height of the nearest track, on the ground or on a viaduct.

use crate::features::Win;
use crate::infra::{
    RAIL_BALLAST, RAIL_PLATFORM, RAIL_SLAB, RAIL_TRACK, RD_GENERAL, RD_MARK_WHITE, RD_MARK_YELLOW, RD_MOTORWAY, RD_NARROW, RD_NATIONAL,
    RD_MEDIAN, RD_PEDESTRIAN, RD_PREF, RD_SIDEWALK, RD_STEPS, RD_TRACK, RD_TRAIL, road_code, road_width,
};
use crate::osm::{F_AREA, F_BRIDGE, F_EMBANKMENT, F_HIGHSPEED, F_ONEWAY, F_SERVICE, F_TUNNEL};
use crate::vfeat::VFeat;
use rayon::prelude::*;
use std::collections::{HashMap, VecDeque};

pub struct Net {
    pub road: Vec<u8>,
    pub roadh: Vec<f32>,
    pub rail: Vec<u8>,
    pub railh: Vec<f32>,
    /// direction of the road through a pixel, degrees 0..179 + 1 (0 = none)
    pub rdir: Vec<u8>,
    /// road ways, road points with a measured width, rail ways, platforms
    pub stats: [usize; 4],
    /// vehicle road pixels painted inside a building footprint (pixel, building)
    pub thru: Vec<(usize, u32)>,
}

/// A ground road line before painting: points, widths and raw heights.
struct RLine {
    pts: Vec<[f64; 2]>,
    s: Vec<f64>,
    vidx: Vec<usize>,
    wl: Vec<f32>,
    wr: Vec<f32>,
    sl: Vec<f32>,
    sr: Vec<f32>,
    ml: Vec<f32>,
    mr: Vec<f32>,
    raw: Vec<f32>,
    water: Vec<f32>,
    edges: usize,
}

// ray-cast targets
const K_EDGE: usize = 1;
const K_WALK: usize = 2;
const K_MEDIAN: usize = 3;

/// Line segments (window pixel coordinates) bucketed in a uniform grid.
struct Grid {
    nc: usize,
    cell: f64,
    start: Vec<u32>,
    ids: Vec<u32>,
    segs: Vec<([f64; 2], [f64; 2], u8)>,
}

impl Grid {
    fn new(n: usize, cell: f64, segs: Vec<([f64; 2], [f64; 2], u8)>) -> Grid {
        let nc = (n as f64 / cell).ceil() as usize;
        let span = |a: f64, b: f64| -> (usize, usize) {
            let lo = (a.min(b) / cell).floor().clamp(0.0, nc as f64 - 1.0) as usize;
            let hi = (a.max(b) / cell).floor().clamp(0.0, nc as f64 - 1.0) as usize;
            (lo, hi)
        };
        let mut cnt = vec![0u32; nc * nc + 1];
        for (a, b, _) in &segs {
            let (x0, x1) = span(a[0], b[0]);
            let (z0, z1) = span(a[1], b[1]);
            for z in z0..=z1 {
                for x in x0..=x1 {
                    cnt[z * nc + x + 1] += 1;
                }
            }
        }
        for i in 1..cnt.len() {
            cnt[i] += cnt[i - 1];
        }
        let mut fill = cnt.clone();
        let mut ids = vec![0u32; *cnt.last().unwrap() as usize];
        for (si, (a, b, _)) in segs.iter().enumerate() {
            let (x0, x1) = span(a[0], b[0]);
            let (z0, z1) = span(a[1], b[1]);
            for z in z0..=z1 {
                for x in x0..=x1 {
                    let c = z * nc + x;
                    ids[fill[c] as usize] = si as u32;
                    fill[c] += 1;
                }
            }
        }
        Grid { nc, cell, start: cnt, ids, segs }
    }

    /// Distance along the ray o + t·d (|d| = 1) to the first segment of each
    /// kind, ignoring hits closer than `tmin`.
    fn cast(&self, o: [f64; 2], d: [f64; 2], tmin: f64, maxd: f64) -> [f64; 4] {
        let mut best = [f64::INFINITY; 4];
        if self.segs.is_empty() {
            return best;
        }
        let nc = self.nc as i64;
        let (mut cx, mut cz) = ((o[0] / self.cell).floor() as i64, (o[1] / self.cell).floor() as i64);
        let step = |v: f64| if v > 0.0 { 1 } else { -1 };
        let (sx, sz) = (step(d[0]), step(d[1]));
        let tnext = |c: i64, o: f64, d: f64| -> f64 {
            if d > 0.0 {
                ((c + 1) as f64 * self.cell - o) / d
            } else if d < 0.0 {
                (c as f64 * self.cell - o) / d
            } else {
                f64::INFINITY
            }
        };
        let (mut tx, mut tz) = (tnext(cx, o[0], d[0]), tnext(cz, o[1], d[1]));
        let dtx = if d[0] != 0.0 { self.cell / d[0].abs() } else { f64::INFINITY };
        let dtz = if d[1] != 0.0 { self.cell / d[1].abs() } else { f64::INFINITY };
        loop {
            if cx >= 0 && cz >= 0 && cx < nc && cz < nc {
                let c = (cz * nc + cx) as usize;
                for &si in &self.ids[self.start[c] as usize..self.start[c + 1] as usize] {
                    let (a, b, k) = self.segs[si as usize];
                    if let Some(t) = ray_seg(o, d, a, b) {
                        if t >= tmin && t <= maxd && t < best[k as usize] {
                            best[k as usize] = t;
                        }
                    }
                }
            }
            if tx < tz {
                if tx > maxd {
                    break;
                }
                cx += sx;
                tx += dtx;
            } else {
                if tz > maxd {
                    break;
                }
                cz += sz;
                tz += dtz;
            }
        }
        best
    }
}

#[inline]
fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

/// Ray o + t·d against segment a–b; returns t.
fn ray_seg(o: [f64; 2], d: [f64; 2], a: [f64; 2], b: [f64; 2]) -> Option<f64> {
    let e = [b[0] - a[0], b[1] - a[1]];
    let den = cross(d, e);
    if den.abs() < 1e-12 {
        return None;
    }
    let ao = [a[0] - o[0], a[1] - o[1]];
    let t = cross(ao, e) / den;
    let u = cross(ao, d) / den;
    (t >= 0.0 && (0.0..=1.0).contains(&u)).then_some(t)
}

/// Densify (keeping the original vertices) to ≤ `step` spacing; returns points
/// and their arc length from the start.
pub fn densify(pts: &[[f64; 2]], step: f64) -> (Vec<[f64; 2]>, Vec<f64>) {
    let mut out = vec![pts[0]];
    let mut s = vec![0.0];
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let d = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        if d < 1e-6 {
            continue;
        }
        let k = (d / step).ceil().max(1.0) as usize;
        let s0 = *s.last().unwrap();
        for i in 1..=k {
            let t = i as f64 / k as f64;
            out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
            s.push(s0 + d * t);
        }
    }
    (out, s)
}

/// Unit direction at each point (central difference) and the right-hand normal.
pub fn normals(p: &[[f64; 2]]) -> Vec<([f64; 2], [f64; 2])> {
    (0..p.len())
        .map(|i| {
            let a = p[i.saturating_sub(1)];
            let b = p[(i + 1).min(p.len() - 1)];
            let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
            let l = (dx * dx + dz * dz).sqrt().max(1e-9);
            let d = [dx / l, dz / l];
            // cross(d, n) > 0 ⇒ positive lateral offsets are on the n side
            (d, [-d[1], d[0]])
        })
        .collect()
}

/// Median over ±k neighbours (NaN-aware).
fn median_filter(v: &[f32], k: usize) -> Vec<f32> {
    (0..v.len())
        .map(|i| {
            let mut w: Vec<f32> = v[i.saturating_sub(k)..(i + k + 1).min(v.len())].iter().copied().filter(|x| x.is_finite()).collect();
            if w.is_empty() {
                return f32::NAN;
            }
            w.sort_by(|a, b| a.partial_cmp(b).unwrap());
            w[w.len() / 2]
        })
        .collect()
}

/// Walk the pixels of a polyline segment, calling f(pixel, lateral u, t) for
/// pixel centres within `wmax` of it (round caps at the ends).
pub fn segment_pixels(n: usize, a: [f64; 2], b: [f64; 2], wmax: f64, mut f: impl FnMut(usize, f64, f64)) {
    let e = [b[0] - a[0], b[1] - a[1]];
    let l2 = e[0] * e[0] + e[1] * e[1];
    if l2 < 1e-12 {
        return;
    }
    let l = l2.sqrt();
    let x0 = ((a[0].min(b[0]) - wmax).floor().max(0.0)) as i64;
    let x1 = ((a[0].max(b[0]) + wmax).ceil().min(n as f64 - 1.0)) as i64;
    let z0 = ((a[1].min(b[1]) - wmax).floor().max(0.0)) as i64;
    let z1 = ((a[1].max(b[1]) + wmax).ceil().min(n as f64 - 1.0)) as i64;
    for z in z0..=z1 {
        for x in x0..=x1 {
            let c = [x as f64 + 0.5, z as f64 + 0.5];
            let ac = [c[0] - a[0], c[1] - a[1]];
            let t = ((ac[0] * e[0] + ac[1] * e[1]) / l2).clamp(0.0, 1.0);
            let q = [a[0] + e[0] * t, a[1] + e[1] * t];
            let d = ((c[0] - q[0]).powi(2) + (c[1] - q[1]).powi(2)).sqrt();
            if d > wmax {
                continue;
            }
            let side = cross(e, ac) / l;
            let u = if t > 0.0 && t < 1.0 { side } else { d.copysign(side) };
            f(z as usize * n + x as usize, u, t);
        }
    }
}

/// 4-connected pixel path of a polyline (rails only link N/S/E/W).
pub fn path4(n: usize, p: &[[f64; 2]], mut f: impl FnMut(usize, usize)) {
    let pix = |q: [f64; 2]| (q[0].floor() as i64, q[1].floor() as i64);
    let mut emit = |x: i64, z: i64, k: usize| {
        if x >= 0 && z >= 0 && (x as usize) < n && (z as usize) < n {
            f(z as usize * n + x as usize, k);
        }
    };
    let Some(&first) = p.first() else { return };
    let (mut cx, mut cz) = pix(first);
    emit(cx, cz, 0);
    for k in 1..p.len() {
        let (tx, tz) = pix(p[k]);
        // step towards the target one axis at a time, choosing the axis whose
        // move keeps the pixel closest to the true segment
        let (a, b) = (p[k - 1], p[k]);
        let e = [b[0] - a[0], b[1] - a[1]];
        let l = (e[0] * e[0] + e[1] * e[1]).sqrt().max(1e-9);
        let dist = |x: i64, z: i64| (cross(e, [x as f64 + 0.5 - a[0], z as f64 + 0.5 - a[1]]) / l).abs();
        let mut guard = 0;
        while (cx, cz) != (tx, tz) && guard < 64 {
            guard += 1;
            let mx = (tx - cx).signum();
            let mz = (tz - cz).signum();
            if mx != 0 && (mz == 0 || dist(cx + mx, cz) <= dist(cx, cz + mz)) {
                cx += mx;
            } else {
                cz += mz;
            }
            emit(cx, cz, k);
        }
    }
}

fn tag<'a>(text: &'a str, k: &str) -> Option<&'a str> {
    text.split(';').find_map(|kv| kv.split_once('=').filter(|(kk, _)| *kk == k).map(|(_, v)| v))
}

/// Drawing order: later (higher) wins where roads overlap.
fn road_prio(code: u8) -> u8 {
    match code {
        RD_TRAIL => 1,
        RD_STEPS => 2,
        RD_SIDEWALK => 3,
        RD_PEDESTRIAN => 4,
        RD_TRACK => 5,
        RD_NARROW => 6,
        RD_GENERAL => 7,
        RD_PREF => 8,
        RD_NATIONAL => 9,
        RD_MOTORWAY => 10,
        _ => 0,
    }
}

/// Grade (rise per m) of the ramps that join a road to the one it meets.
fn road_grade(code: u8) -> f32 {
    match code {
        RD_MOTORWAY => 0.05,
        RD_NATIONAL => 0.06,
        RD_PREF => 0.07,
        RD_GENERAL => 0.08,
        RD_NARROW => 0.1,
        RD_TRACK => 0.12,
        RD_STEPS => 0.6,
        _ => 0.15,
    }
}

/// Smoothing radius (m) of the surface profile along a road.
fn road_smooth_r(code: u8) -> f64 {
    match code {
        RD_MOTORWAY => 30.0,
        RD_NATIONAL => 20.0,
        RD_PREF => 15.0,
        RD_GENERAL => 8.0,
        RD_NARROW => 5.0,
        RD_TRACK => 4.0,
        RD_PEDESTRIAN | RD_SIDEWALK => 3.0,
        RD_TRAIL => 2.0,
        _ => 0.0,
    }
}

pub struct Inputs<'a> {
    pub roads: &'a [VFeat],
    pub rails: &'a [VFeat],
    pub platforms: &'a [VFeat],
    pub rdedg: &'a [VFeat],
    pub rdcompt: &'a [VFeat],
    /// profiled bridges/tunnels (for elevated platforms)
    pub structs: &'a [VFeat],
    /// building ids of the window (roads keep out of them)
    pub bldg: &'a [u32],
}

pub fn rasterize(w: &Win, hgt: &[f32], cls: &[u8], inp: &Inputs) -> Net {
    use crate::heightmap::{K_INLAND, K_OUT, K_SEA};
    let n = w.n;
    let to_px = |p: &[[f64; 2]]| -> Vec<[f64; 2]> { p.iter().map(|q| [q[0] - w.x0 as f64, q[1] - w.z0 as f64]).collect() };
    let inside = |q: [f64; 2], m: f64| q[0] >= -m && q[1] >= -m && q[0] < n as f64 + m && q[1] < n as f64 + m;
    let sample = |q: [f64; 2]| -> f32 {
        let (x, z) = (q[0].floor() as i64, q[1].floor() as i64);
        if x < 0 || z < 0 || x >= n as i64 || z >= n as i64 {
            return f32::NAN;
        }
        let i = z as usize * n + x as usize;
        // water columns hold the sea floor / water surface, not ground
        if cls[i] == K_OUT || cls[i] == K_SEA || cls[i] == K_INLAND { f32::NAN } else { hgt[i] }
    };
    // the water surface under a point (for ways lying entirely over water: piers)
    let water_top = |q: [f64; 2]| -> f32 {
        let (x, z) = (q[0].floor() as i64, q[1].floor() as i64);
        if x < 0 || z < 0 || x >= n as i64 || z >= n as i64 {
            return f32::NAN;
        }
        let i = z as usize * n + x as usize;
        match cls[i] {
            K_INLAND => hgt[i],
            K_SEA => 0.0,
            _ => f32::NAN,
        }
    };
    // ways the network pass made structures (bridges, viaducts, tunnels, also
    // by layer or between structure ways) are drawn from their profiles instead
    let road_ids: std::collections::HashSet<u64> = inp.structs.iter().filter(|f| f.nums[3] < 0.5).map(|f| f.id).collect();
    let rail_ids: std::collections::HashSet<u64> = inp.structs.iter().filter(|f| f.nums[3] >= 0.5).map(|f| f.id).collect();
    let mut stats = [0usize; 4];

    // ---- road edge / sidewalk line index
    let mut segs = Vec::new();
    for f in inp.rdedg {
        // 1 真幅道路, 2 庭園路等, 3 徒歩道, 5 建設中 (4 = in tunnels)
        if !matches!(f.class, 1 | 2 | 3 | 5) {
            continue;
        }
        for p in &f.parts {
            let p = to_px(p);
            for s in p.windows(2) {
                if inside(s[0], 40.0) || inside(s[1], 40.0) {
                    segs.push((s[0], s[1], K_EDGE as u8));
                }
            }
        }
    }
    for f in inp.rdcompt {
        // 1 歩道, 2 分離帯
        let k = match f.class {
            1 => K_WALK,
            2 => K_MEDIAN,
            _ => continue,
        };
        for p in &f.parts {
            let p = to_px(p);
            for s in p.windows(2) {
                if inside(s[0], 40.0) || inside(s[1], 40.0) {
                    segs.push((s[0], s[1], k as u8));
                }
            }
        }
    }
    let grid = Grid::new(n, 8.0, segs);

    // ---- roads: the geometry, widths and raw heights of every way; the
    // heights solved over the whole network (profile.rs: split ways are one
    // line, side roads meet the road they join, bridge and tunnel ends hold);
    // then painted in order of priority
    let mut road = vec![0u8; n * n];
    let mut roadh = vec![f32::NAN; n * n];
    let mut ways: Vec<(u8, &VFeat)> = Vec::new();
    for f in inp.roads {
        if road_ids.contains(&f.id) || f.class == 24 || f.parts.is_empty() {
            continue;
        }
        if tag(&f.text, "footway") == Some("crossing") {
            continue; // painted onto the carriageway below
        }
        ways.push((road_code(f.class, &f.text), f));
    }
    // painted in order of priority, the higher class last (it keeps a
    // crossing); ramps (*_link) before every main road, so a ramp ending on a
    // road or running into it never paints over that road's carriageway
    let link = |f: &VFeat| matches!(f.class, 2 | 4 | 6 | 8 | 10);
    ways.sort_by_key(|(c, f)| (if link(f) { 13 } else { 2 * road_prio(*c) }, f.id));
    let bldg_at = |q: [f64; 2]| -> bool {
        let (x, z) = (q[0].floor() as i64, q[1].floor() as i64);
        x >= 0 && z >= 0 && x < n as i64 && z < n as i64 && inp.bldg[z as usize * n + x as usize] != 0
    };
    let near = |q: [f64; 2]| inside(q, 60.0);
    let lines: Vec<Option<RLine>> = ways
        .par_iter()
        .map(|&(code, f)| {
            if f.area || f.flags & F_AREA != 0 {
                return None;
            }
            let px = to_px(&f.parts[0]);
            let (pts, s, vidx) = crate::profile::densify(&px, 2.0);
            if pts.len() < 2 {
                return None;
            }
            let half = (road_width(f.class, f.nums, &f.text) / 2.0) as f64;
            // how far to look for the road edge (half widths of real roads of the class)
            let dmax = match f.class {
                1 | 2 => 15.0f64,
                3..=6 => 25.0,
                7 | 8 => 18.0,
                9 | 10 => 14.0,
                11 | 12 | 16 | 23 => 10.0,
                13 | 14 => 7.0,
                15 => 6.0,
                _ => 5.0,
            }
            .max(half + 1.0);
            let use_edges = !matches!(code, RD_TRAIL | RD_STEPS | RD_SIDEWALK);
            let oneway = f.flags & F_ONEWAY != 0;
            let nm = normals(&pts);
            let m = pts.len();
            let (mut wl, mut wr, mut sl, mut sr) = (vec![half as f32; m], vec![half as f32; m], vec![f32::NAN; m], vec![f32::NAN; m]);
            let (mut ml, mut mr) = (vec![f32::NAN; m], vec![f32::NAN; m]);
            let mut edges = 0usize;
            // how far each side may reach before a building
            let (mut capl, mut capr) = (vec![f32::INFINITY; m], vec![f32::INFINITY; m]);
            for i in 0..m {
                if !near(pts[i]) {
                    continue;
                }
                let (_, nr) = nm[i];
                // a building beside the road bounds it (a road running through
                // a building is left to the tile stage)
                let in_bldg = bldg_at(pts[i]);
                for (side, dir) in [(0, nr), (1, [-nr[0], -nr[1]])] {
                    let mut wside = if side == 0 { wr[i] } else { wl[i] };
                    if use_edges {
                        let hit = grid.cast(pts[i], dir, 0.6, dmax);
                        // a median strip bounds a carriageway of a dual road (one-way
                        // ways); on a single two-way way it is the planted centre
                        let mut we = hit[K_EDGE];
                        let wm = hit[K_MEDIAN];
                        if oneway {
                            we = we.min(wm);
                        } else if wm < 4.0 && wm < we {
                            if side == 0 { mr[i] = wm as f32 } else { ml[i] = wm as f32 }
                        }
                        if we.is_finite() && we >= 1.0 {
                            wside = we as f32;
                            edges += 1;
                            // an edge beyond the usual width only where the ground
                            // stays level out to it (outside the cities the edges are
                            // mapped coarsely; a road does not reach up a hillside)
                            let c0 = sample(pts[i]);
                            if wside as f64 > half && c0.is_finite() {
                                let mut t = half;
                                while t < wside as f64 {
                                    let hv = sample([pts[i][0] + dir[0] * (t + 0.5), pts[i][1] + dir[1] * (t + 0.5)]);
                                    if hv.is_finite() && (hv - c0).abs() > 2.5 {
                                        break;
                                    }
                                    t += 0.5;
                                }
                                wside = wside.min(t as f32);
                            }
                            let ws = hit[K_WALK];
                            if ws.is_finite() && ws < we - 0.8 {
                                if side == 0 { sr[i] = ws as f32 } else { sl[i] = ws as f32 }
                            }
                        }
                    }
                    if !in_bldg {
                        let mut t = 0.5;
                        while t < dmax.max(wside as f64) + 0.5 {
                            if bldg_at([pts[i][0] + dir[0] * t, pts[i][1] + dir[1] * t]) {
                                let cap = ((t - 0.5) as f32).max(1.0);
                                if side == 0 { capr[i] = cap } else { capl[i] = cap }
                                break;
                            }
                            t += 0.5;
                        }
                    }
                    if side == 0 { wr[i] = wside } else { wl[i] = wside }
                }
            }
            if use_edges {
                wl = median_filter(&wl, 5);
                wr = median_filter(&wr, 5);
            }
            for i in 0..m {
                wl[i] = wl[i].min(capl[i]);
                wr[i] = wr[i].min(capr[i]);
            }
            if use_edges {
                // a sidewalk line must be seen at most points nearby to count
                let keep = |v: &[f32]| -> Vec<f32> {
                    let med = median_filter(v, 3);
                    (0..v.len())
                        .map(|i| {
                            let lo = i.saturating_sub(3);
                            let hi = (i + 4).min(v.len());
                            let seen = v[lo..hi].iter().filter(|x| x.is_finite()).count();
                            if seen * 2 > hi - lo { med[i] } else { f32::NAN }
                        })
                        .collect()
                };
                sl = keep(&sl);
                sr = keep(&sr);
                ml = keep(&ml);
                mr = keep(&mr);
            }
            // raw surface height: the median of the land across the road
            let mut raw = vec![f32::NAN; m];
            for i in 0..m {
                if !near(pts[i]) {
                    continue;
                }
                let (_, nr) = nm[i];
                let mut v: Vec<f32> = Vec::new();
                let (a, b) = (-(wl[i].max(0.5) as f64), wr[i].max(0.5) as f64);
                let mut o = a;
                while o <= b + 1e-6 {
                    let hv = sample([pts[i][0] + nr[0] * o, pts[i][1] + nr[1] * o]);
                    if hv.is_finite() {
                        v.push(hv);
                    }
                    o += 1.0;
                }
                if !v.is_empty() {
                    v.sort_by(|x, y| x.partial_cmp(y).unwrap());
                    raw[i] = v[v.len() / 2];
                }
            }
            let water: Vec<f32> = pts.iter().map(|&q| water_top(q)).collect();
            Some(RLine { pts, s, vidx, wl, wr, sl, sr, ml, mr, raw, water, edges })
        })
        .collect();
    // bridge and tunnel ends hold the ground roads that meet them
    let mut pins: HashMap<crate::profile::Key, crate::profile::Pin> = HashMap::new();
    for f in inp.structs.iter().filter(|f| f.nums[3] < 0.5) {
        let (Some(p), Some(z)) = (f.parts.first(), f.zs.first()) else { continue };
        let vehicle = matches!(f.class, 1..=16 | 23);
        for (q, &h) in p.iter().zip(z) {
            pins.entry(crate::profile::key(*q)).or_insert(crate::profile::Pin { h, vehicle });
        }
    }
    let mut line_of = vec![usize::MAX; ways.len()];
    let mut pways: Vec<crate::profile::PWay> = Vec::new();
    for (wi, l) in lines.iter().enumerate() {
        let Some(l) = l else { continue };
        let (code, f) = ways[wi];
        line_of[wi] = pways.len();
        pways.push(crate::profile::PWay {
            id: f.id,
            verts: &f.parts[0],
            vidx: l.vidx.clone(),
            s: l.s.clone(),
            raw: l.raw.clone(),
            water: l.water.clone(),
            rank: road_prio(code),
            radius: road_smooth_r(code),
            grade: road_grade(code),
            name: tag(&f.text, "ref").or_else(|| tag(&f.text, "name")).unwrap_or("").to_string(),
            vehicle: matches!(f.class, 1..=16 | 23),
        });
    }
    let solved = crate::profile::solve(&pways, &pins);
    drop(pways);
    // the decks and tunnel floors of road structures (pixel -> structure,
    // lowest and highest surface): a ground road is not painted where a
    // structure it leads onto covers it at about its height (the end of a
    // bridge, the mouth of a tunnel), so the deck does not sit on a strip of
    // road; structures crossing over or under it do not count
    let mut under: HashMap<usize, Vec<(u32, f32, f32)>> = HashMap::new();
    let mut struct_at: HashMap<crate::profile::Key, Vec<u32>> = HashMap::new();
    for (si, f) in inp.structs.iter().enumerate().filter(|(_, f)| f.nums[3] < 0.5) {
        let si = si as u32;
        if let Some(p) = f.parts.first() {
            for q in p {
                let v = struct_at.entry(crate::profile::key(*q)).or_default();
                if !v.contains(&si) {
                    v.push(si);
                }
            }
        }
        let (Some(p), Some(z)) = (f.parts.first(), f.zs.first()) else { continue };
        if p.len() < 2 || z.len() != p.len() {
            continue;
        }
        let hw = (road_width(f.class, f.nums, &f.text) + 1.0) as f64 / 2.0;
        let px = to_px(p);
        let last = px.len() - 2;
        for k in 0..px.len() - 1 {
            if !inside(px[k], 40.0) && !inside(px[k + 1], 40.0) {
                continue;
            }
            segment_pixels(n, px[k], px[k + 1], hw, |q, _, t| {
                // not past the two ends (the road leading on keeps its markings up to them)
                if (k == 0 && t <= 0.0) || (k == last && t >= 1.0) {
                    return;
                }
                let h = z[k] + (z[k + 1] - z[k]) * t as f32;
                let v = under.entry(q).or_default();
                match v.iter_mut().find(|e| e.0 == si) {
                    Some(e) => {
                        e.1 = e.1.min(h);
                        e.2 = e.2.max(h);
                    }
                    None => v.push((si, h, h)),
                }
            });
        }
    }
    // nearest segment of the current way per pixel, so the round cap of one
    // segment at a bend does not override the next segment's projection
    let mut owner = vec![u32::MAX; n * n];
    let mut odist = vec![f32::INFINITY; n * n];
    let mut rdir = vec![0u8; n * n];
    // the surface under a lane marking (a zebra crossing's gaps get it back)
    let mut carriage = vec![0u8; n * n];
    // the way painted on each pixel, its distance from that way's centre line, its direction
    let mut pway = vec![u32::MAX; n * n];
    let mut pd = vec![f32::INFINITY; n * n];
    let mut pdir = vec![0u8; n * n];
    // vehicle road pixels painted inside a building (the tile stage decides
    // whether the road or the building gives way)
    let mut thru: Vec<(usize, u32)> = Vec::new();
    // end vertices of every way: a way that ends where another begins (a road
    // split into ways, a ramp joining) continues it
    let ends: Vec<(crate::profile::Key, crate::profile::Key)> = ways
        .iter()
        .map(|(_, f)| match f.parts.first() {
            Some(pt) if !pt.is_empty() => (crate::profile::key(pt[0]), crate::profile::key(pt[pt.len() - 1])),
            _ => ((i64::MIN, i64::MIN), (i64::MAX, i64::MAX)),
        })
        .collect();
    let continues = |a: usize, b: usize| {
        let ((a0, a1), (b0, b1)) = (ends[a], ends[b]);
        a0 == b0 || a0 == b1 || a1 == b0 || a1 == b1
    };
    for (wi, (code, f)) in ways.iter().enumerate() {
        let code = *code;
        stats[0] += 1;
        if f.area || f.flags & F_AREA != 0 {
            // pedestrian squares etc.: ground height, plain surface
            w.fill(&f.parts, |p| {
                if cls[p] != K_OUT {
                    road[p] = if code == RD_PEDESTRIAN || code == RD_SIDEWALK { code } else { RD_PEDESTRIAN };
                    roadh[p] = hgt[p];
                }
            });
            continue;
        }
        let Some(l) = &lines[wi] else { continue };
        stats[1] += l.edges;
        let h = &solved[line_of[wi]];
        let (pts, s) = (&l.pts, &l.s);
        let (wl, wr, sl, sr, ml, mr) = (&l.wl, &l.wr, &l.sl, &l.sr, &l.ml, &l.mr);
        let m = pts.len();
        let oneway = f.flags & F_ONEWAY != 0;
        let vehicle_way = matches!(f.class, 1..=16 | 23);
        // the structures this way leads onto
        let mut onto: Vec<u32> = f.parts[0].iter().filter_map(|q| struct_at.get(&crate::profile::key(*q))).flatten().copied().collect();
        onto.sort_unstable();
        onto.dedup();
        // markings
        let lanes = f.nums[1];
        let wi = wi as u32;
        for k in 0..m - 1 {
            if !near(pts[k]) && !near(pts[k + 1]) {
                continue;
            }
            let wmax = (wl[k].max(wr[k]).max(wl[k + 1]).max(wr[k + 1]) as f64) + 0.5;
            let ang = (pts[k + 1][1] - pts[k][1]).atan2(pts[k + 1][0] - pts[k][0]).to_degrees().rem_euclid(180.0);
            let wf = (ang.to_radians().cos().abs() + ang.to_radians().sin().abs()) as f32;
            let dir = (ang.round() as u16 % 180) as u8 + 1;
            let vehicle = matches!(code, RD_MOTORWAY | RD_NATIONAL | RD_PREF | RD_GENERAL | RD_NARROW | RD_TRACK);
            segment_pixels(n, pts[k], pts[k + 1], wmax, |p, u, t| {
                if cls[p] == K_OUT {
                    return;
                }
                if vehicle {
                    rdir[p] = dir;
                }
                let d = u.abs() as f32;
                if owner[p] == wi && odist[p] <= d {
                    return;
                }
                // the round end of the way: surface only, and only where no
                // other road is (it would paint across the road it ends on)
                let cap = (k == 0 && t <= 0.0) || (k == m - 2 && t >= 1.0);
                if cap && road[p] != 0 && pway[p] != wi {
                    return;
                }
                // another carriageway is already painted here. One running
                // alongside (a ramp beside the main line, the two halves of a
                // divided road) keeps the pixels nearer its own centre line,
                // markings and all, so overlapping parallel roads meet along the
                // line between them; one crossing makes this a junction, where
                // edge and centre lines stop (they would cut across the mouth of
                // the side road and the box of the crossing)
                let other = pway[p] != wi && pway[p] != u32::MAX && matches!(road[p], RD_MOTORWAY | RD_NATIONAL | RD_PREF | RD_GENERAL | RD_NARROW | RD_MARK_WHITE | RD_MARK_YELLOW);
                // (two roads at different levels, a ramp in a cutting beside the
                // street above it, do not meet either: the nearer one keeps it)
                let hh = h[k] + (h[k + 1] - h[k]) * t as f32;
                let parallel = other && {
                    let a = (pdir[p] as i32 - dir as i32).rem_euclid(180);
                    let a = a.min(180 - a);
                    a <= 20 || (a <= 60 && continues(pway[p] as usize, wi as usize)) || (roadh[p] - hh).abs() > 2.0
                };
                if parallel && pd[p] <= d {
                    return;
                }
                let junction = other && !parallel;
                owner[p] = wi;
                odist[p] = d;
                let lerp = |v: &[f32]| v[k] + (v[k + 1] - v[k]) * t as f32;
                let (ws, ss, wo, so) = if u >= 0.0 { (lerp(wr), lerp(sr), lerp(wl), lerp(sl)) } else { (lerp(wl), lerp(sl), lerp(wr), lerp(sr)) };
                let au = u.abs() as f32;
                if au > ws {
                    return;
                }
                if !hh.is_finite() {
                    return;
                }
                if !onto.is_empty() && under.get(&p).is_some_and(|v| v.iter().any(|&(si, lo, hi)| hh > lo - 3.0 && hh < hi + 3.0 && onto.binary_search(&si).is_ok())) {
                    return;
                }
                let mut c = code;
                let med = if u >= 0.0 { lerp(mr) } else { lerp(ml) };
                if ss.is_finite() && au >= ss {
                    c = RD_SIDEWALK;
                } else if med.is_finite() && au < med {
                    c = RD_MEDIAN;
                } else if matches!(code, RD_MOTORWAY | RD_NATIONAL | RD_PREF | RD_GENERAL) {
                    // carriageway between the curbs (or edges)
                    let inner_s = if ss.is_finite() { ss } else { ws };
                    let inner_o = if so.is_finite() { so } else { wo };
                    let (r_in, l_in) = if u >= 0.0 { (inner_s, inner_o) } else { (inner_o, inner_s) };
                    let hc = (r_in + l_in) / 2.0;
                    let v = u as f32 - (r_in - l_in) / 2.0;
                    let sl_ = s[k] + (s[k + 1] - s[k]) * t;
                    c = marking(code, f.class, v, hc, sl_, oneway, lanes, wf).unwrap_or(code);
                    if junction || cap {
                        c = code;
                    }
                }
                if vehicle_way && inp.bldg[p] != 0 {
                    thru.push((p, inp.bldg[p]));
                }
                road[p] = c;
                carriage[p] = if matches!(c, RD_MARK_WHITE | RD_MARK_YELLOW) { code } else { c };
                roadh[p] = hh;
                pway[p] = wi;
                pd[p] = d;
                pdir[p] = dir;
            });
        }
    }
    // zebra crossings painted onto carriageways: 1 m bars along the road, 1 m
    // apart across it; the bars follow the grid axis nearest to the road, so a
    // crossing of a diagonal road is a clean ladder of pixels, not a checkerboard
    for f in inp.roads {
        if tag(&f.text, "footway") != Some("crossing") || road_ids.contains(&f.id) || f.parts.is_empty() || f.area {
            continue;
        }
        let (pts, _) = densify(&to_px(&f.parts[0]), 1.0);
        for k in 0..pts.len().saturating_sub(1) {
            // the crossing runs across the road: alternate along it
            let across_x = (pts[k + 1][0] - pts[k][0]).abs() >= (pts[k + 1][1] - pts[k][1]).abs();
            segment_pixels(n, pts[k], pts[k + 1], 2.0, |p, _, _| {
                if matches!(road[p], RD_MOTORWAY | RD_NATIONAL | RD_PREF | RD_GENERAL | RD_NARROW | RD_MARK_WHITE | RD_MARK_YELLOW) {
                    let bar = if across_x { (p % n) % 2 == 0 } else { (p / n) % 2 == 0 };
                    if bar {
                        road[p] = RD_MARK_WHITE;
                    } else if road[p] == RD_MARK_WHITE || road[p] == RD_MARK_YELLOW {
                        road[p] = if carriage[p] != 0 { carriage[p] } else { RD_GENERAL };
                    }
                }
            });
        }
    }

    // ---- rails: raw heights per line, solved over the rail network like the
    // roads (a line split at stations and switches is one profile, sidings meet
    // the main line, bridge and tunnel ends hold), then laid
    let mut rail = vec![0u8; n * n];
    let mut railh = vec![f32::NAN; n * n];
    let mut rails: Vec<&VFeat> = inp.rails.iter().filter(|f| !rail_ids.contains(&f.id) && !f.area && !f.parts.is_empty() && f.class != 5).collect();
    // sidings first, main lines last (they win where they overlap)
    rails.sort_by_key(|f| (f.flags & F_SERVICE == 0, f.flags & F_HIGHSPEED != 0, f.id));
    struct RailLine {
        pts: Vec<[f64; 2]>,
        s: Vec<f64>,
        vidx: Vec<usize>,
        raw: Vec<f32>,
        water: Vec<f32>,
    }
    let rlines: Vec<Option<RailLine>> = rails
        .iter()
        .map(|f| {
            let (pts, s, vidx) = crate::profile::densify(&to_px(&f.parts[0]), 1.0);
            if pts.len() < 2 || !pts.iter().any(|&q| inside(q, 60.0)) {
                return None;
            }
            let hs = f.flags & F_HIGHSPEED != 0;
            let tram = f.class == 7;
            let tracks = (f.nums[0].round() as i32).clamp(1, 8);
            let spacing = if hs { 4.3 } else { 4.0 };
            let nm = normals(&pts);
            let span = (tracks as f64 - 1.0) * spacing / 2.0 + 1.5;
            let mut raw = vec![f32::NAN; pts.len()];
            for i in 0..pts.len() {
                if !inside(pts[i], 80.0) {
                    continue;
                }
                let (_, nr) = nm[i];
                let mut v: Vec<f32> = Vec::new();
                let mut o = -span;
                while o <= span + 1e-6 {
                    let q = [pts[i][0] + nr[0] * o, pts[i][1] + nr[1] * o];
                    let (x, z) = (q[0].floor() as i64, q[1].floor() as i64);
                    // trams run on the road surface
                    let rv = if x >= 0 && z >= 0 && (x as usize) < n && (z as usize) < n { roadh[z as usize * n + x as usize] } else { f32::NAN };
                    let hv = if tram && rv.is_finite() { rv } else { sample(q) };
                    if hv.is_finite() {
                        v.push(hv);
                    }
                    o += 1.0;
                }
                if !v.is_empty() {
                    v.sort_by(|x, y| x.partial_cmp(y).unwrap());
                    raw[i] = v[v.len() / 2];
                }
            }
            let water: Vec<f32> = pts.iter().map(|&q| water_top(q)).collect();
            Some(RailLine { pts, s, vidx, raw, water })
        })
        .collect();
    let mut rpins: HashMap<crate::profile::Key, crate::profile::Pin> = HashMap::new();
    for f in inp.structs.iter().filter(|f| f.nums[3] >= 0.5) {
        let (Some(p), Some(z)) = (f.parts.first(), f.zs.first()) else { continue };
        for (q, &h) in p.iter().zip(z) {
            rpins.entry(crate::profile::key(*q)).or_insert(crate::profile::Pin { h, vehicle: false });
        }
    }
    let mut rline_of = vec![usize::MAX; rails.len()];
    let mut rways: Vec<crate::profile::PWay> = Vec::new();
    for (ri, l) in rlines.iter().enumerate() {
        let Some(l) = l else { continue };
        let f = rails[ri];
        let (tram, service, hs) = (f.class == 7, f.flags & F_SERVICE != 0, f.flags & F_HIGHSPEED != 0);
        rline_of[ri] = rways.len();
        rways.push(crate::profile::PWay {
            id: f.id,
            verts: &f.parts[0],
            vidx: l.vidx.clone(),
            s: l.s.clone(),
            raw: l.raw.clone(),
            water: l.water.clone(),
            rank: if tram { 0 } else if service { 1 } else if hs { 3 } else { 2 },
            radius: if tram { 6.0 } else if service { 15.0 } else { 40.0 },
            grade: if tram { 0.06 } else if hs { 0.02 } else { 0.03 },
            name: tag(&f.text, "name").unwrap_or("").to_string(),
            vehicle: false,
        });
    }
    let rsolved = crate::profile::solve(&rways, &rpins);
    drop(rways);
    for (ri, f) in rails.iter().enumerate() {
        let Some(l) = &rlines[ri] else { continue };
        stats[2] += 1;
        let h = &rsolved[rline_of[ri]];
        let pts = &l.pts;
        let hs = f.flags & F_HIGHSPEED != 0;
        let tram = f.class == 7;
        let tracks = (f.nums[0].round() as i32).clamp(1, 8);
        let spacing = if hs { 4.3 } else { 4.0 };
        let nm = normals(pts);
        let _ = &l.s;
        for k in 0..tracks {
            let off = (k as f64 - (tracks as f64 - 1.0) / 2.0) * spacing;
            let tp: Vec<[f64; 2]> = pts.iter().zip(&nm).map(|(q, (_, nr))| [q[0] + nr[0] * off, q[1] + nr[1] * off]).collect();
            if !tram {
                let bw = if hs { 1.7 } else { 1.6 };
                for j in 0..tp.len() - 1 {
                    if !inside(tp[j], 4.0) {
                        continue;
                    }
                    segment_pixels(n, tp[j], tp[j + 1], bw, |p, _, t| {
                        let hh = h[j] + (h[j + 1] - h[j]) * t as f32;
                        if hh.is_finite() && cls[p] != K_OUT && rail[p] != RAIL_TRACK {
                            rail[p] = if hs { RAIL_SLAB } else { RAIL_BALLAST };
                            railh[p] = hh;
                        }
                    });
                }
            }
            path4(n, &tp, |p, j| {
                let hh = h[j.min(h.len() - 1)];
                if hh.is_finite() && cls[p] != K_OUT {
                    rail[p] = RAIL_TRACK;
                    railh[p] = hh;
                }
            });
        }
    }
    // the bed between parallel tracks (stations, yards): ballast across a gap
    // of up to 6 m between two track beds at about the same height, so the
    // ground between the tracks is not lawn or pavement
    {
        let src = rail.clone();
        let bed = |q: usize| matches!(src[q], RAIL_TRACK | RAIL_BALLAST);
        let free = |q: usize| src[q] == 0 && road[q] == 0 && inp.bldg[q] == 0 && !matches!(cls[q], K_OUT | K_SEA | K_INLAND);
        for (stride, step, lines) in [(n, 1usize, n), (1usize, n, n)] {
            for line in 0..lines {
                let at = |k: usize| line * stride + k * step;
                let mut last: Option<usize> = None;
                for k in 0..n {
                    let p = at(k);
                    if bed(p) {
                        if let Some(l) = last {
                            let (a, b) = (at(l), p);
                            if k - l > 1 && k - l <= 7 && (railh[a] - railh[b]).abs() <= 1.5 {
                                for j in l + 1..k {
                                    let q = at(j);
                                    rail[q] = RAIL_BALLAST;
                                    railh[q] = railh[a] + (railh[b] - railh[a]) * (j - l) as f32 / (k - l) as f32;
                                }
                            }
                        }
                        last = Some(k);
                    } else if !free(p) {
                        last = None;
                    }
                }
            }
        }
    }

    // ---- platforms: height of the nearest track (ground or viaduct)
    let mut seed_h = vec![f32::NAN; n * n];
    for p in 0..n * n {
        if rail[p] == RAIL_TRACK {
            seed_h[p] = railh[p];
        }
    }
    for f in inp.structs {
        if f.nums[3] < 0.5 || f.flags & (F_BRIDGE | F_EMBANKMENT) == 0 {
            continue;
        }
        let (Some(p), Some(z)) = (f.parts.first(), f.zs.first()) else { continue };
        let px = to_px(p);
        for k in 0..px.len().saturating_sub(1) {
            if !inside(px[k], 20.0) {
                continue;
            }
            segment_pixels(n, px[k], px[k + 1], 0.7, |q, _, t| {
                seed_h[q] = z[k] + (z[k + 1] - z[k]) * t as f32;
            });
        }
    }
    let mut plat: Vec<usize> = Vec::new();
    for f in inp.platforms {
        // (bus stops are public-transport platforms too)
        let bus = f.text.contains("highway=bus_stop") || f.text.contains("bus=yes") || f.text.contains("amenity=bus_station");
        let rail_platform = f.text.contains("railway=platform") || (f.text.contains("public_transport=platform") && !f.text.contains("highway=platform") && !bus);
        if !rail_platform || f.flags & F_TUNNEL != 0 || f.nums[2] < 0.0 || tag(&f.text, "level").is_some_and(|l| l.starts_with('-')) {
            continue;
        }
        stats[3] += 1;
        if f.area {
            w.fill(&f.parts, |p| plat.push(p));
        } else {
            for part in &f.parts {
                let (pts, _) = densify(&to_px(part), 1.0);
                for k in 0..pts.len().saturating_sub(1) {
                    segment_pixels(n, pts[k], pts[k + 1], 1.5, |p, _, _| plat.push(p));
                }
            }
        }
    }
    if !plat.is_empty() {
        // multi-source BFS from the tracks, 15 m at most
        let mut dist = vec![u8::MAX; n * n];
        let mut q = VecDeque::new();
        let (mut zx0, mut zx1, mut zz0, mut zz1) = (usize::MAX, 0, usize::MAX, 0);
        for &p in &plat {
            zx0 = zx0.min(p % n);
            zx1 = zx1.max(p % n);
            zz0 = zz0.min(p / n);
            zz1 = zz1.max(p / n);
        }
        let (bx0, bx1, bz0, bz1) = (zx0.saturating_sub(16), (zx1 + 16).min(n - 1), zz0.saturating_sub(16), (zz1 + 16).min(n - 1));
        for z in bz0..=bz1 {
            for x in bx0..=bx1 {
                let p = z * n + x;
                if seed_h[p].is_finite() {
                    dist[p] = 0;
                    q.push_back(p);
                }
            }
        }
        let mut hfrom = seed_h.clone();
        while let Some(p) = q.pop_front() {
            let d = dist[p];
            if d >= 15 {
                continue;
            }
            let (x, z) = (p % n, p / n);
            for (dx, dz) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                let (xx, zz) = (x as i64 + dx, z as i64 + dz);
                if xx < bx0 as i64 || zz < bz0 as i64 || xx > bx1 as i64 || zz > bz1 as i64 {
                    continue;
                }
                let pp = zz as usize * n + xx as usize;
                if dist[pp] == u8::MAX {
                    dist[pp] = d + 1;
                    hfrom[pp] = hfrom[p];
                    q.push_back(pp);
                }
            }
        }
        for p in plat {
            if hfrom[p].is_finite() && rail[p] != RAIL_TRACK && cls[p] != K_OUT {
                rail[p] = RAIL_PLATFORM;
                railh[p] = hfrom[p];
            }
        }
    }
    Net { road, roadh, rail, railh, rdir, stats, thru }
}

/// Lane marking at lateral offset `v` from the carriageway centre (half width `hc`).
/// `wf` (|cos| + |sin| of the road's direction, 1 to √2) widens the 1 m lines
/// of a diagonal road so that their pixels meet side to side, not corner to
/// corner: a continuous line instead of a row of dots.
pub fn marking(code: u8, class: u16, v: f32, hc: f32, s: f64, oneway: bool, lanes: f32, wf: f32) -> Option<u8> {
    let av = v.abs();
    let half = 0.5 * wf;
    let on = |pos: f32| (v - pos).abs() < half;
    match code {
        RD_MOTORWAY => {
            if hc < 2.5 {
                return None;
            }
            if (av - (hc - 1.0)).abs() < half {
                return Some(RD_MARK_WHITE);
            }
            let n = if lanes >= 1.0 { lanes.round() } else { ((2.0 * hc - 3.0) / 3.6).round().max(1.0) };
            let lw = (2.0 * hc - 1.5) / n;
            // long dashes: 1 m wide lines of several lanes at 8 m on, 12 m off
            // would read as hatching across the carriageway
            let dashed = s.rem_euclid(20.0) < 14.0;
            for k in 1..n as i32 {
                if on(-hc + 0.75 + k as f32 * lw) && dashed {
                    return Some(RD_MARK_WHITE);
                }
            }
            None
        }
        RD_NATIONAL | RD_PREF | RD_GENERAL => {
            if 2.0 * hc < 5.5 {
                return None;
            }
            if code != RD_GENERAL && (av - (hc - 0.5)).abs() < half && av < hc {
                return Some(RD_MARK_WHITE); // 外側線
            }
            if oneway || class == 2 || class == 4 || class == 6 || class == 8 {
                return None; // links and one-way roads: no centre line
            }
            if av < half {
                return match code {
                    RD_NATIONAL => Some(RD_MARK_YELLOW),
                    RD_PREF => Some(RD_MARK_WHITE),
                    _ => (s.rem_euclid(10.0) < 5.0).then_some(RD_MARK_WHITE),
                };
            }
            None
        }
        _ => None,
    }
}
