//! Global pass over OSM roads and railways: vertical profiles of bridges and
//! tunnels. Consecutive bridge (or tunnel) ways are chained so a long viaduct
//! split into many ways gets one smooth deck. Ground-level ways are not
//! profiled here (their surface comes from the DEM in the tile stage).
//!
//! Output `vec_net_struct.bin`: each bridge/tunnel way, densified to ≤2 m,
//! with the deck/floor surface height (m, same datum as the heightmap) per
//! vertex in `zs[0]`. class/flags/nums/text are copied from the source way;
//! nums[3] = 1 for rail, 0 for road.

use crate::ingest::TILE_MARGIN;
use crate::osm::{F_BRIDGE, F_HIGHSPEED, F_TUNNEL};
use crate::vfeat::{self, TiledReader, VFeat};
use crate::world::TileSet;
use anyhow::Result;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::Path;

fn key(p: [f64; 2]) -> (i64, i64) {
    ((p[0] * 1000.0).round() as i64, (p[1] * 1000.0).round() as i64)
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

fn densify(pts: &[[f64; 2]], step: f64) -> Vec<[f64; 2]> {
    let mut out = vec![pts[0]];
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let d = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let n = (d / step).ceil().max(1.0) as usize;
        for i in 1..=n {
            let t = i as f64 / n as f64;
            out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
        }
    }
    out
}

fn cumlen(p: &[[f64; 2]]) -> Vec<f64> {
    let mut s = vec![0.0];
    for w in p.windows(2) {
        let d = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
        s.push(s.last().unwrap() + d);
    }
    s
}

struct Way {
    f: VFeat,
    rail: bool,
}

/// Deck/floor profile of a chain (concatenated polyline, ≤2 m spacing).
fn profile(ts: &TileSet, pts: &[[f64; 2]], bridge: bool, rail: bool, layer: f32, highspeed: bool) -> Result<Vec<f32>> {
    let s = cumlen(pts);
    let l = *s.last().unwrap();
    let ground: Vec<f32> = pts.iter().map(|p| ts.mean_height(p[0], p[1], 0).ok().flatten().unwrap_or(0.0)).collect();
    // abutments / portals: mean ground in a 5×5 m window at both ends
    let h0 = ts.mean_height(pts[0][0], pts[0][1], 2)?.unwrap_or(ground[0]) as f64;
    let n = pts.len() - 1;
    let h1 = ts.mean_height(pts[n][0], pts[n][1], 2)?.unwrap_or(ground[n]) as f64;
    let lin: Vec<f64> = s.iter().map(|&si| if l > 0.0 { h0 + (h1 - h0) * si / l } else { h0 }).collect();
    if !bridge {
        return Ok(lin.iter().map(|v| *v as f32).collect());
    }
    // clearance under the deck, ramping up from the abutments (10 % grade)
    let cmax = if rail { if highspeed { 9.0 } else { 7.0 } } else { 5.5 } + (layer.max(1.0) as f64 - 1.0) * 5.0;
    let env: Vec<f64> = s
        .iter()
        .zip(&ground)
        .map(|(&si, &g)| {
            let c = (si.min(l - si) * 0.10).min(cmax);
            g as f64 + c
        })
        .collect();
    // widen the envelope (±20 m max) then smooth (±15 m mean) so the deck stays flat over piers
    let win = |r: f64, si: f64, sj: f64| (si - sj).abs() <= r;
    let mut emax = vec![0f64; env.len()];
    for i in 0..env.len() {
        let mut m = f64::MIN;
        let mut j = i;
        while j > 0 && win(20.0, s[i], s[j - 1]) {
            j -= 1;
        }
        while j < env.len() && win(20.0, s[i], s[j]) {
            m = m.max(env[j]);
            j += 1;
        }
        emax[i] = m;
    }
    let mut h = vec![0f64; env.len()];
    for i in 0..env.len() {
        let (mut sum, mut cnt) = (0.0, 0.0);
        let mut j = i;
        while j > 0 && win(15.0, s[i], s[j - 1]) {
            j -= 1;
        }
        while j < env.len() && win(15.0, s[i], s[j]) {
            sum += emax[j];
            cnt += 1.0;
            j += 1;
        }
        h[i] = lin[i].max(sum / cnt);
    }
    Ok(h.iter().map(|v| *v as f32).collect())
}

pub fn run(work: &Path, hm_dir: &Path) -> Result<()> {
    let t0 = std::time::Instant::now();
    let mut ways: Vec<Way> = Vec::new();
    for (file, rail) in [("vec_osm_road.bin", false), ("vec_osm_rail.bin", true)] {
        for f in load_all(&work.join(file))? {
            if f.parts.first().is_some_and(|p| p.len() >= 2) && !f.area {
                ways.push(Way { f, rail });
            }
        }
    }
    eprintln!("network: {} ways in {:.1}s", ways.len(), t0.elapsed().as_secs_f64());
    // which endpoints touch a non-structure way (of the same kind)
    let mut ground_nodes: HashSet<(bool, (i64, i64))> = HashSet::new();
    let mut struct_ends: HashMap<(bool, u16, (i64, i64)), Vec<usize>> = HashMap::new();
    for (i, w) in ways.iter().enumerate() {
        let p = &w.f.parts[0];
        let kind = w.f.flags & (F_BRIDGE | F_TUNNEL);
        if kind == 0 {
            for q in p {
                ground_nodes.insert((w.rail, key(*q)));
            }
        } else {
            for q in [p[0], *p.last().unwrap()] {
                struct_ends.entry((w.rail, kind, key(q))).or_default().push(i);
            }
        }
    }
    // chain structure ways through endpoints shared by exactly two of them and no ground way
    let mut used = vec![false; ways.len()];
    let mut chains: Vec<Vec<(usize, bool)>> = Vec::new(); // (way, reversed)
    let struct_ids: Vec<usize> = (0..ways.len()).filter(|&i| ways[i].f.flags & (F_BRIDGE | F_TUNNEL) != 0).collect();
    let next = |w: usize, end: [f64; 2], used: &Vec<bool>| -> Option<usize> {
        let kind = ways[w].f.flags & (F_BRIDGE | F_TUNNEL);
        let k = (ways[w].rail, kind, key(end));
        if ground_nodes.contains(&(ways[w].rail, k.2)) {
            return None;
        }
        let v = struct_ends.get(&k)?;
        if v.len() != 2 {
            return None;
        }
        let o = if v[0] == w { v[1] } else { v[0] };
        (!used[o] && o != w).then_some(o)
    };
    for &start in &struct_ids {
        if used[start] {
            continue;
        }
        used[start] = true;
        // grow backwards from the start's first point, then forwards from its last point
        let mut chain: Vec<(usize, bool)> = vec![(start, false)];
        let mut tail = *ways[start].f.parts[0].last().unwrap();
        let mut cur = start;
        while let Some(o) = next(cur, tail, &used) {
            used[o] = true;
            let p = &ways[o].f.parts[0];
            let rev = key(p[0]) != key(tail);
            tail = if rev { p[0] } else { *p.last().unwrap() };
            chain.push((o, rev));
            cur = o;
        }
        let mut head = ways[start].f.parts[0][0];
        cur = start;
        while let Some(o) = next(cur, head, &used) {
            used[o] = true;
            let p = &ways[o].f.parts[0];
            // o must end at `head`, so it is reversed if its last point isn't head
            let rev = key(*p.last().unwrap()) != key(head);
            head = if rev { *p.last().unwrap() } else { p[0] };
            chain.insert(0, (o, rev));
            cur = o;
        }
        chains.push(chain);
    }
    eprintln!("network: {} structure ways in {} chains ({:.1}s)", struct_ids.len(), chains.len(), t0.elapsed().as_secs_f64());
    // spatially sort chains so the heightmap block cache stays warm
    chains.sort_by_key(|c| {
        let p = ways[c[0].0].f.parts[0][0];
        ((p[1] / 10240.0).floor() as i64, (p[0] / 10240.0).floor() as i64)
    });
    let ts = TileSet::new(hm_dir);
    let out: Vec<Vec<VFeat>> = chains
        .par_iter()
        .map(|chain| -> Result<Vec<VFeat>> {
            // concatenate (densified) geometry, remembering each way's span
            let mut pts: Vec<[f64; 2]> = Vec::new();
            let mut spans = Vec::new();
            for &(w, rev) in chain {
                let mut p = densify(&ways[w].f.parts[0], 2.0);
                if rev {
                    p.reverse();
                }
                let start = if pts.is_empty() { 0 } else { pts.len() - 1 };
                if pts.is_empty() {
                    pts.extend_from_slice(&p);
                } else {
                    pts.extend_from_slice(&p[1..]);
                }
                spans.push((w, rev, start, pts.len() - 1));
            }
            let f0 = &ways[chain[0].0];
            let bridge = f0.f.flags & F_BRIDGE != 0;
            let layer = chain.iter().map(|&(w, _)| ways[w].f.nums[2]).fold(f32::MIN, f32::max);
            let hs = chain.iter().any(|&(w, _)| ways[w].f.flags & F_HIGHSPEED != 0);
            let z = profile(&ts, &pts, bridge, f0.rail, layer, hs)?;
            Ok(spans
                .into_iter()
                .map(|(w, rev, a, b)| {
                    let mut p = pts[a..=b].to_vec();
                    let mut zz = z[a..=b].to_vec();
                    if rev {
                        p.reverse();
                        zz.reverse();
                    }
                    let src = &ways[w].f;
                    let mut nums = src.nums;
                    nums[3] = if ways[w].rail { 1.0 } else { 0.0 };
                    VFeat { class: src.class, flags: src.flags, nums, id: src.id, text: src.text.clone(), area: false, parts: vec![p], zs: vec![zz] }
                })
                .collect())
        })
        .collect::<Result<_>>()?;
    let feats: Vec<VFeat> = out.into_iter().flatten().collect();
    vfeat::write_tiled(&work.join("vec_net_struct.bin"), &feats, TILE_MARGIN)?;
    eprintln!("network: {} bridge/tunnel ways profiled in {:.1}s", feats.len(), t0.elapsed().as_secs_f64());
    Ok(())
}
