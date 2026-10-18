//! Height profiles of ground ways (roads or railways) solved together, so
//! the network joins up: a way split into several OSM ways is smoothed as one
//! line ("stroke": the ways that continue each other straight on at their
//! ends), and where lines meet, the lesser one is bent to the height of the
//! one it joins (the through road keeps its profile, a side road ramps to it).
//! Bridges and tunnels come first: a ground way that meets one takes the
//! height of its end (solved over the whole network in network.rs).

use std::collections::HashMap;

pub type Key = (i64, i64);

/// Vertex key (mm) shared by ways meeting at a vertex.
pub fn key(p: [f64; 2]) -> Key {
    ((p[0] * 1000.0).round() as i64, (p[1] * 1000.0).round() as i64)
}

/// One way: its points (densified, with the original vertices among them),
/// the raw height of each point (NaN: unknown), and how it is smoothed.
pub struct PWay<'a> {
    pub id: u64,
    /// original vertices (world coordinates) and their index in `s`/`raw`
    pub verts: &'a [[f64; 2]],
    pub vidx: Vec<usize>,
    /// arc length of each point from the way's start
    pub s: Vec<f64>,
    pub raw: Vec<f32>,
    /// water surface under each point (ways lying entirely over water: piers)
    pub water: Vec<f32>,
    /// higher wins at junctions
    pub rank: u8,
    /// smoothing radius (m) and the grade the junction ramps keep
    pub radius: f64,
    pub grade: f32,
    /// name or ref: a way continues the one with the same name first
    pub name: String,
    pub vehicle: bool,
}

/// A fixed height at a vertex: bridge and tunnel ends.
#[derive(Clone, Copy)]
pub struct Pin {
    pub h: f32,
    /// the structure carries vehicles: only vehicle ways are held by it
    pub vehicle: bool,
}

fn unit_dir(a: [f64; 2], b: [f64; 2]) -> Option<[f64; 2]> {
    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
    let l = dx.hypot(dz);
    (l > 1e-6).then(|| [dx / l, dz / l])
}

/// Direction in which a way leaves its start (`end` false) or its end.
fn out_dir(v: &[[f64; 2]], end: bool) -> Option<[f64; 2]> {
    let n = v.len();
    if end {
        (1..n).rev().find_map(|k| unit_dir(v[n - 1], v[k - 1]))
    } else {
        (1..n).find_map(|k| unit_dir(v[0], v[k]))
    }
}

/// Moving average over ±r(i) m along the line, of the finite values.
fn smooth_var(v: &[f32], s: &[f64], r: &[f64]) -> Vec<f32> {
    let n = v.len();
    let mut cs = vec![0f64; n + 1];
    let mut cc = vec![0u32; n + 1];
    for i in 0..n {
        let ok = v[i].is_finite();
        cs[i + 1] = cs[i] + if ok { v[i] as f64 } else { 0.0 };
        cc[i + 1] = cc[i] + ok as u32;
    }
    (0..n)
        .map(|i| {
            let lo = s.partition_point(|&x| x < s[i] - r[i]);
            let hi = s.partition_point(|&x| x <= s[i] + r[i]);
            let c = cc[hi] - cc[lo];
            if c == 0 { f32::NAN } else { ((cs[hi] - cs[lo]) / c as f64) as f32 }
        })
        .collect()
}

/// Gaps (water) take the heights on both sides; a line that is all water lies
/// one block above the water.
fn fill(h: &mut [f32], water: &[f32]) {
    if !h.iter().any(|v| v.is_finite()) {
        for (v, w) in h.iter_mut().zip(water) {
            *v = w + 1.0;
        }
    }
    let known: Vec<usize> = (0..h.len()).filter(|&i| h[i].is_finite()).collect();
    let (Some(&a), Some(&b)) = (known.first(), known.last()) else { return };
    for i in 0..a {
        h[i] = h[a];
    }
    for i in b + 1..h.len() {
        h[i] = h[b];
    }
    for w in known.windows(2) {
        let (p, q) = (w[0], w[1]);
        for i in p + 1..q {
            h[i] = h[p] + (h[q] - h[p]) * (i - p) as f32 / (q - p) as f32;
        }
    }
}

/// Correction along a line from fixed values at some points: linear between
/// pins whose ramps meet, else a ramp of length `ramp(c)` down to zero on
/// each side of a pin.
fn corrections(s: &[f64], pins: &[(usize, f32)], ramp: impl Fn(f32) -> f64) -> Vec<f32> {
    let mut c = vec![0f32; s.len()];
    if pins.is_empty() {
        return c;
    }
    let mut ps: Vec<(usize, f32)> = pins.to_vec();
    ps.sort_by_key(|p| p.0);
    ps.dedup_by_key(|p| p.0);
    let at = |i: usize, (k, ck): (usize, f32), l: f64| -> f32 {
        let d = (s[i] - s[k]).abs();
        if d >= l { 0.0 } else { ck * (1.0 - d / l) as f32 }
    };
    // before the first and after the last pin
    let (f, l) = (ps[0], *ps.last().unwrap());
    for i in 0..f.0 {
        c[i] = at(i, f, ramp(f.1));
    }
    for i in l.0..s.len() {
        c[i] = at(i, l, ramp(l.1));
    }
    for w in ps.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (la, lb) = (ramp(a.1), ramp(b.1));
        let gap = s[b.0] - s[a.0];
        for i in a.0..=b.0 {
            c[i] = if gap <= la + lb {
                let t = if gap > 0.0 { ((s[i] - s[a.0]) / gap) as f32 } else { 0.0 };
                a.1 + (b.1 - a.1) * t
            } else {
                at(i, a, la) + at(i, b, lb)
            };
        }
    }
    c
}

/// Final heights of every point of every way.
pub fn solve(ways: &[PWay], pins: &HashMap<Key, Pin>) -> Vec<Vec<f32>> {
    let nw = ways.len();
    // ---- strokes: at each vertex where ways end, pair the ends that continue
    // each other (deflection < 60°; same name, then same rank, then straightest)
    let mut ends: HashMap<Key, Vec<(usize, bool, [f64; 2])>> = HashMap::new();
    for (wi, w) in ways.iter().enumerate() {
        if w.verts.len() < 2 {
            continue;
        }
        for end in [false, true] {
            if let Some(d) = out_dir(w.verts, end) {
                let p = if end { w.verts[w.verts.len() - 1] } else { w.verts[0] };
                ends.entry(key(p)).or_default().push((wi, end, d));
            }
        }
    }
    // link[wi][end] = (other way, its end)
    let mut link: Vec<[Option<(usize, bool)>; 2]> = vec![[None, None]; nw];
    let mut keys: Vec<&Key> = ends.keys().collect();
    keys.sort();
    for k in keys {
        let e = &ends[k];
        if e.len() < 2 {
            continue;
        }
        let mut cand: Vec<(f64, usize, usize)> = Vec::new();
        for a in 0..e.len() {
            for b in a + 1..e.len() {
                let (wa, wb) = (&ways[e[a].0], &ways[e[b].0]);
                if e[a].0 == e[b].0 {
                    continue; // a closed loop's own two ends
                }
                let cos = -(e[a].2[0] * e[b].2[0] + e[a].2[1] * e[b].2[1]);
                if cos < 0.5 {
                    continue;
                }
                let score = (wa.name != wb.name || wa.name.is_empty()) as u8 as f64 * 2.0 + (wa.rank != wb.rank) as u8 as f64 + (1.0 - cos);
                cand.push((score, a, b));
            }
        }
        cand.sort_by(|x, y| x.0.total_cmp(&y.0).then((ways[e[x.1].0].id, ways[e[x.2].0].id).cmp(&(ways[e[y.1].0].id, ways[e[y.2].0].id))));
        let mut used = vec![false; e.len()];
        for (_, a, b) in cand {
            if used[a] || used[b] {
                continue;
            }
            used[a] = true;
            used[b] = true;
            let (wa, ea, _) = e[a];
            let (wb, eb, _) = e[b];
            link[wa][ea as usize] = Some((wb, eb));
            link[wb][eb as usize] = Some((wa, ea));
        }
    }
    // ---- walk the strokes: (way, forward) in order
    let mut order: Vec<usize> = (0..nw).collect();
    order.sort_by_key(|&w| ways[w].id);
    let mut seen = vec![false; nw];
    let mut strokes: Vec<Vec<(usize, bool)>> = Vec::new();
    for &w0 in &order {
        if seen[w0] {
            continue;
        }
        // back to the first way of the stroke (or once round a ring)
        let (mut w, mut fwd) = (w0, true);
        let mut guard = 0;
        loop {
            // leaving w backwards: through its start if forward, else its end
            let back_end = !fwd;
            let Some((pw, pe)) = link[w][back_end as usize] else { break };
            if pw == w0 || guard > nw {
                break;
            }
            guard += 1;
            // entering pw at pe going backwards: forward if pe is its end
            w = pw;
            fwd = pe;
        }
        let mut s: Vec<(usize, bool)> = Vec::new();
        let (mut cw, mut cf) = (w, fwd);
        loop {
            if seen[cw] {
                break;
            }
            seen[cw] = true;
            s.push((cw, cf));
            let exit_end = cf; // forward: leaves through its end
            let Some((nw2, ne)) = link[cw][exit_end as usize] else { break };
            cw = nw2;
            cf = !ne; // entered at its start: forward
        }
        strokes.push(s);
    }
    // ---- per stroke: concatenated raw heights, smoothed, gaps filled
    struct St {
        parts: Vec<(usize, bool, usize)>, // way, forward, offset of its first point in the stroke
        s: Vec<f64>,
        h: Vec<f32>,
        rank: u8,
        grade: f32,
        radius: f64,
        first_id: u64,
        vehicle: bool,
    }
    let mut sts: Vec<St> = Vec::with_capacity(strokes.len());
    for st in &strokes {
        let (mut s, mut raw, mut water, mut rr) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut parts = Vec::new();
        let mut base = 0.0;
        for &(wi, fwd) in st {
            let w = &ways[wi];
            let m = w.s.len();
            let len = *w.s.last().unwrap_or(&0.0);
            // the first point repeats the last one of the previous way
            let skip = !s.is_empty();
            parts.push((wi, fwd, s.len() - skip as usize));
            for k in skip as usize..m {
                let j = if fwd { k } else { m - 1 - k };
                s.push(base + if fwd { w.s[j] } else { len - w.s[j] });
                raw.push(w.raw[j]);
                water.push(w.water[j]);
                rr.push(w.radius);
            }
            base += len;
        }
        let mut h = smooth_var(&raw, &s, &rr);
        fill(&mut h, &water);
        let rank = st.iter().map(|&(w, _)| ways[w].rank).max().unwrap_or(0);
        let grade = st.iter().map(|&(w, _)| ways[w].grade).fold(f32::MAX, f32::min);
        let radius = st.iter().map(|&(w, _)| ways[w].radius).fold(0.0, f64::max);
        let first_id = st.iter().map(|&(w, _)| ways[w].id).min().unwrap_or(0);
        let vehicle = st.iter().any(|&(w, _)| ways[w].vehicle);
        sts.push(St { parts, s, h, rank, grade, radius, first_id, vehicle });
    }
    // ---- junctions: the higher stroke keeps its heights, lower ones meet it
    let mut ord: Vec<usize> = (0..sts.len()).collect();
    ord.sort_by(|&a, &b| sts[b].rank.cmp(&sts[a].rank).then(sts[a].first_id.cmp(&sts[b].first_id)));
    let mut node_h: HashMap<Key, f32> = HashMap::new();
    for si in ord {
        // stroke index of each original vertex
        let mut vk: Vec<(usize, Key, bool)> = Vec::new(); // (stroke point, key, way is vehicle)
        for &(wi, fwd, off) in &sts[si].parts {
            let w = &ways[wi];
            let m = w.s.len();
            for (j, &p) in w.verts.iter().enumerate() {
                let k = w.vidx[j];
                let sp = off + if fwd { k } else { m - 1 - k };
                vk.push((sp, key(p), w.vehicle));
            }
        }
        let st = &mut sts[si];
        let mut fixed: Vec<(usize, f32)> = Vec::new();
        for &(sp, k, veh) in &vk {
            let target = match (pins.get(&k), node_h.get(&k)) {
                (Some(p), _) if !p.vehicle || veh => Some(p.h),
                (_, Some(&h)) => Some(h),
                _ => None,
            };
            if let Some(t) = target {
                let c = t - st.h[sp];
                // a gap of more than a storey is not a junction at grade (a
                // structure end above or below the ground road)
                if c.abs() <= 6.0 {
                    fixed.push((sp, c));
                }
            }
        }
        if !fixed.is_empty() {
            let (r, g) = (st.radius, st.grade.max(0.005) as f64);
            let corr = corrections(&st.s, &fixed, |c| (r * 2.0).max(c.abs() as f64 / g).max(6.0));
            for (h, c) in st.h.iter_mut().zip(corr) {
                *h += c;
            }
        }
        for &(sp, k, _) in &vk {
            node_h.entry(k).or_insert(st.h[sp]);
        }
        let _ = st.vehicle;
    }
    // ---- back to the ways
    let mut out: Vec<Vec<f32>> = ways.iter().map(|w| vec![f32::NAN; w.s.len()]).collect();
    for st in &sts {
        for &(wi, fwd, off) in &st.parts {
            let m = ways[wi].s.len();
            for k in 0..m {
                let j = if fwd { k } else { m - 1 - k };
                out[wi][j] = st.h[off + k];
            }
        }
    }
    out
}

/// Densify keeping the original vertices: points, arc lengths and the index
/// of every original vertex among the points.
pub fn densify(pts: &[[f64; 2]], step: f64) -> (Vec<[f64; 2]>, Vec<f64>, Vec<usize>) {
    let mut out = vec![pts[0]];
    let mut s = vec![0.0];
    let mut vidx = vec![0usize];
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let d = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        if d < 1e-6 {
            vidx.push(out.len() - 1);
            continue;
        }
        let k = (d / step).ceil().max(1.0) as usize;
        let s0 = *s.last().unwrap();
        for i in 1..=k {
            let t = i as f64 / k as f64;
            out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
            s.push(s0 + d * t);
        }
        vidx.push(out.len() - 1);
    }
    (out, s, vidx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn way<'a>(id: u64, v: &'a [[f64; 2]], f: impl Fn([f64; 2]) -> f32, rank: u8, name: &str) -> PWay<'a> {
        let (pts, s, vidx) = densify(v, 2.0);
        let raw: Vec<f32> = pts.iter().map(|&p| f(p)).collect();
        PWay { id, verts: v, vidx, water: vec![f32::NAN; s.len()], s, raw, rank, radius: 10.0, grade: 0.08, name: name.into(), vehicle: true }
    }

    #[test]
    fn split_way_is_one_line_and_side_road_meets_it() {
        // a road on a 10 % slope split in two at x = 100; a side road joins at x = 100
        let slope = |p: [f64; 2]| (p[0] * 0.1) as f32;
        let a = [[0.0, 0.0], [100.0, 0.0]];
        let b = [[100.0, 0.0], [200.0, 0.0]];
        let c = [[100.0, 0.0], [100.0, 80.0]];
        // the side road climbs away from the junction (its own raw heights say +3 m there)
        let side = |p: [f64; 2]| (10.0 + 3.0 + p[1] * 0.05) as f32;
        let ways = vec![way(1, &a, slope, 5, "r1"), way(2, &b, slope, 5, "r1"), way(3, &c, side, 3, "")];
        let h = solve(&ways, &HashMap::new());
        // continuous across the split, on the slope (no one-sided smoothing bias)
        let ea = *h[0].last().unwrap();
        let sb = h[1][0];
        assert!((ea - sb).abs() < 1e-4, "split: {ea} vs {sb}");
        assert!((ea - 10.0).abs() < 0.05, "split height {ea}");
        // the side road starts at the through road's height
        assert!((h[2][0] - ea).abs() < 1e-4, "side road {} vs {}", h[2][0], ea);
    }

    #[test]
    fn pinned_end_ramps() {
        let flat = |_p: [f64; 2]| 5.0f32;
        let a = [[0.0, 0.0], [200.0, 0.0]];
        let ways = vec![way(1, &a, flat, 5, "")];
        let mut pins = HashMap::new();
        pins.insert(key([200.0, 0.0]), Pin { h: 7.0, vehicle: true });
        let h = solve(&ways, &pins);
        assert!((h[0].last().unwrap() - 7.0).abs() < 1e-4);
        // a 2 m correction ramps over 2 / 0.08 = 25 m
        let i = ways[0].s.iter().position(|&s| s >= 170.0).unwrap();
        assert!((h[0][i] - 5.0).abs() < 1e-3, "{}", h[0][i]);
    }
}
