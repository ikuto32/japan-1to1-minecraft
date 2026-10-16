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
use crate::osm::{F_AREA, F_BRIDGE, F_HIGHSPEED, F_ONEWAY, F_SERVICE, F_TUNNEL};
use crate::vfeat::VFeat;
use std::collections::VecDeque;

pub struct Net {
    pub road: Vec<u8>,
    pub roadh: Vec<f32>,
    pub rail: Vec<u8>,
    pub railh: Vec<f32>,
    /// direction of the road through a pixel, degrees 0..179 + 1 (0 = none)
    pub rdir: Vec<u8>,
    /// road ways, road points with a measured width, rail ways, platforms
    pub stats: [usize; 4],
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

/// Moving average over an arc-length window ±r, ignoring NaN.
fn smooth(v: &[f32], s: &[f64], r: f64) -> Vec<f32> {
    if r <= 0.0 {
        return v.to_vec();
    }
    let n = v.len();
    let mut out = vec![f32::NAN; n];
    let (mut lo, mut hi) = (0usize, 0usize);
    let (mut sum, mut cnt) = (0f64, 0usize);
    for i in 0..n {
        while hi < n && s[hi] <= s[i] + r {
            if v[hi].is_finite() {
                sum += v[hi] as f64;
                cnt += 1;
            }
            hi += 1;
        }
        while s[lo] < s[i] - r {
            if v[lo].is_finite() {
                sum -= v[lo] as f64;
                cnt -= 1;
            }
            lo += 1;
        }
        if cnt > 0 {
            out[i] = (sum / cnt as f64) as f32;
        }
    }
    out
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
}

pub fn rasterize(w: &Win, hgt: &[f32], cls: &[u8], inp: &Inputs) -> Net {
    use crate::heightmap::K_OUT;
    let n = w.n;
    let to_px = |p: &[[f64; 2]]| -> Vec<[f64; 2]> { p.iter().map(|q| [q[0] - w.x0 as f64, q[1] - w.z0 as f64]).collect() };
    let inside = |q: [f64; 2], m: f64| q[0] >= -m && q[1] >= -m && q[0] < n as f64 + m && q[1] < n as f64 + m;
    let sample = |q: [f64; 2]| -> f32 {
        let (x, z) = (q[0].floor() as i64, q[1].floor() as i64);
        if x < 0 || z < 0 || x >= n as i64 || z >= n as i64 {
            return f32::NAN;
        }
        let i = z as usize * n + x as usize;
        if cls[i] == K_OUT { f32::NAN } else { hgt[i] }
    };
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

    // ---- roads
    let mut road = vec![0u8; n * n];
    let mut roadh = vec![f32::NAN; n * n];
    let mut ways: Vec<(u8, &VFeat)> = Vec::new();
    for f in inp.roads {
        if f.flags & (F_BRIDGE | F_TUNNEL) != 0 || f.class == 24 || f.parts.is_empty() {
            continue;
        }
        if tag(&f.text, "footway") == Some("crossing") {
            continue; // painted onto the carriageway below
        }
        ways.push((road_code(f.class, &f.text), f));
    }
    ways.sort_by_key(|(c, f)| (road_prio(*c), f.id));
    // nearest segment of the current way per pixel, so the round cap of one
    // segment at a bend does not override the next segment's projection
    let mut owner = vec![u32::MAX; n * n];
    let mut odist = vec![f32::INFINITY; n * n];
    let mut rdir = vec![0u8; n * n];
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
        let px = to_px(&f.parts[0]);
        let (pts, s) = densify(&px, 2.0);
        if pts.len() < 2 {
            continue;
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
        let near = |q: [f64; 2]| inside(q, 60.0);
        if use_edges {
            for i in 0..m {
                if !near(pts[i]) {
                    continue;
                }
                let (_, nr) = nm[i];
                for (side, dir) in [(0, nr), (1, [-nr[0], -nr[1]])] {
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
                        if side == 0 { wr[i] = we as f32 } else { wl[i] = we as f32 }
                        stats[1] += 1;
                        let ws = hit[K_WALK];
                        if ws.is_finite() && ws < we - 0.8 {
                            if side == 0 { sr[i] = ws as f32 } else { sl[i] = ws as f32 }
                        }
                    }
                }
            }
            wl = median_filter(&wl, 5);
            wr = median_filter(&wr, 5);
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
        // surface height: cross-section median, smoothed along the road
        let mut h = vec![f32::NAN; m];
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
                h[i] = v[v.len() / 2];
            }
        }
        let h = smooth(&h, &s, road_smooth_r(code));
        // markings
        let lanes = f.nums[1];
        let wi = wi as u32;
        for k in 0..m - 1 {
            if !near(pts[k]) && !near(pts[k + 1]) {
                continue;
            }
            let wmax = (wl[k].max(wr[k]).max(wl[k + 1]).max(wr[k + 1]) as f64) + 0.5;
            let ang = (pts[k + 1][1] - pts[k][1]).atan2(pts[k + 1][0] - pts[k][0]).to_degrees().rem_euclid(180.0);
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
                owner[p] = wi;
                odist[p] = d;
                let lerp = |v: &[f32]| v[k] + (v[k + 1] - v[k]) * t as f32;
                let (ws, ss, wo, so) = if u >= 0.0 { (lerp(&wr), lerp(&sr), lerp(&wl), lerp(&sl)) } else { (lerp(&wl), lerp(&sl), lerp(&wr), lerp(&sr)) };
                let au = u.abs() as f32;
                if au > ws {
                    return;
                }
                let hh = h[k] + (h[k + 1] - h[k]) * t as f32;
                if !hh.is_finite() {
                    return;
                }
                let mut c = code;
                let med = if u >= 0.0 { lerp(&mr) } else { lerp(&ml) };
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
                    c = marking(code, f.class, v, hc, sl_, oneway, lanes).unwrap_or(code);
                }
                road[p] = c;
                roadh[p] = hh;
            });
        }
    }
    // zebra crossings painted onto carriageways
    for f in inp.roads {
        if tag(&f.text, "footway") != Some("crossing") || f.flags & (F_BRIDGE | F_TUNNEL) != 0 || f.parts.is_empty() || f.area {
            continue;
        }
        let (pts, s) = densify(&to_px(&f.parts[0]), 1.0);
        for k in 0..pts.len().saturating_sub(1) {
            segment_pixels(n, pts[k], pts[k + 1], 2.0, |p, _, t| {
                if matches!(road[p], RD_MOTORWAY | RD_NATIONAL | RD_PREF | RD_GENERAL | RD_NARROW | RD_MARK_WHITE | RD_MARK_YELLOW) {
                    let sl_ = s[k] + (s[k + 1] - s[k]) * t;
                    if (sl_.floor() as i64).rem_euclid(2) == 0 {
                        road[p] = RD_MARK_WHITE;
                    } else if road[p] == RD_MARK_WHITE || road[p] == RD_MARK_YELLOW {
                        road[p] = RD_GENERAL;
                    }
                }
            });
        }
    }

    // ---- rails
    let mut rail = vec![0u8; n * n];
    let mut railh = vec![f32::NAN; n * n];
    let mut rails: Vec<&VFeat> = inp.rails.iter().filter(|f| f.flags & (F_BRIDGE | F_TUNNEL) == 0 && !f.area && !f.parts.is_empty() && f.class != 5).collect();
    // sidings first, main lines last (they win where they overlap)
    rails.sort_by_key(|f| (f.flags & F_SERVICE == 0, f.flags & F_HIGHSPEED != 0, f.id));
    for f in rails {
        let (pts, s) = densify(&to_px(&f.parts[0]), 1.0);
        if pts.len() < 2 || !pts.iter().any(|&q| inside(q, 60.0)) {
            continue;
        }
        stats[2] += 1;
        let hs = f.flags & F_HIGHSPEED != 0;
        let tram = f.class == 7;
        let tracks = (f.nums[0].round() as i32).clamp(1, 8);
        let spacing = if hs { 4.3 } else { 4.0 };
        let nm = normals(&pts);
        let span = (tracks as f64 - 1.0) * spacing / 2.0 + 1.5;
        let mut h = vec![f32::NAN; pts.len()];
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
                h[i] = v[v.len() / 2];
            }
        }
        let r = if tram { 6.0 } else if f.flags & F_SERVICE != 0 { 15.0 } else { 40.0 };
        let h = smooth(&h, &s, r);
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

    // ---- platforms: height of the nearest track (ground or viaduct)
    let mut seed_h = vec![f32::NAN; n * n];
    for p in 0..n * n {
        if rail[p] == RAIL_TRACK {
            seed_h[p] = railh[p];
        }
    }
    for f in inp.structs {
        if f.nums[3] < 0.5 || f.flags & F_BRIDGE == 0 {
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
        let rail_platform = f.text.contains("railway=platform") || (f.text.contains("public_transport=platform") && !f.text.contains("highway=platform"));
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
    Net { road, roadh, rail, railh, rdir, stats }
}

/// Lane marking at lateral offset `v` from the carriageway centre (half width `hc`).
fn marking(code: u8, class: u16, v: f32, hc: f32, s: f64, oneway: bool, lanes: f32) -> Option<u8> {
    let av = v.abs();
    let on = |pos: f32| (v - pos).abs() < 0.5;
    match code {
        RD_MOTORWAY => {
            if hc < 2.5 {
                return None;
            }
            if av >= hc - 1.5 && av < hc - 0.5 {
                return Some(RD_MARK_WHITE);
            }
            let n = if lanes >= 1.0 { lanes.round() } else { ((2.0 * hc - 3.0) / 3.6).round().max(1.0) };
            let lw = (2.0 * hc - 1.5) / n;
            let dashed = s.rem_euclid(20.0) < 8.0;
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
            if code != RD_GENERAL && av >= hc - 1.0 && av < hc {
                return Some(RD_MARK_WHITE); // 外側線
            }
            if oneway || class == 2 || class == 4 || class == 6 || class == 8 {
                return None; // links and one-way roads: no centre line
            }
            if av < 0.5 {
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
