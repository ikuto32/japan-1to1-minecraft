//! Roads, railways, platforms, bridges, tunnels and water structures.
//!
//! Ground-level roads/rails come from the feature rasters (road/roadh,
//! rail/railh); bridges and tunnels from the profiled OSM ways
//! (`vec_net_struct.bin`). Road surfaces are quantized to half blocks
//! (bottom slabs) so normal grades can be walked without jumping.

use crate::blocks::{BlockId, b};
use crate::osm::{F_BRIDGE, F_EMBANKMENT, F_HIGHSPEED, F_TUNNEL};
use crate::rgen::{CF_BLDG, CF_NO_PLANT, CF_NO_TREE, CF_RAIL, CF_ROAD, CF_STRUCT, CF_WATER, Col, Gen, Ids, Overlay, SEA_Y};
use crate::vfeat::VFeat;
use std::collections::HashMap;

// ground road surface codes (road_*.tif)
pub const RD_MOTORWAY: u8 = 1;
pub const RD_NATIONAL: u8 = 2; // trunk / primary (国道)
pub const RD_PREF: u8 = 3; // secondary (県道)
pub const RD_GENERAL: u8 = 4; // tertiary / unclassified / residential
pub const RD_NARROW: u8 = 5; // service / living street / narrow paved
pub const RD_SIDEWALK: u8 = 6;
pub const RD_PEDESTRIAN: u8 = 7; // paved footway / plaza
pub const RD_TRAIL: u8 = 8; // path / hiking trail
pub const RD_TRACK: u8 = 9; // 林道 / farm track (gravel)
pub const RD_STEPS: u8 = 10;
pub const RD_MARK_WHITE: u8 = 11;
pub const RD_MARK_YELLOW: u8 = 12;
pub const RD_MEDIAN: u8 = 13; // planted median strip

/// (full block, slab) for a road code.
pub fn road_blocks(code: u8) -> (BlockId, Option<BlockId>) {
    match code {
        RD_MOTORWAY => (b("polished_blackstone"), Some(b("polished_blackstone_slab[type=bottom,waterlogged=false]"))),
        RD_NATIONAL => (b("polished_deepslate"), Some(b("polished_deepslate_slab[type=bottom,waterlogged=false]"))),
        RD_PREF => (b("stone"), Some(b("stone_slab[type=bottom,waterlogged=false]"))),
        RD_GENERAL => (b("andesite"), Some(b("andesite_slab[type=bottom,waterlogged=false]"))),
        RD_MARK_WHITE => (b("smooth_quartz"), Some(b("smooth_quartz_slab[type=bottom,waterlogged=false]"))),
        RD_MARK_YELLOW => (b("smooth_sandstone"), Some(b("smooth_sandstone_slab[type=bottom,waterlogged=false]"))),
        RD_MEDIAN => (b("grass_block[snowy=false]"), None),
        RD_NARROW => (b("smooth_stone"), Some(b("smooth_stone_slab[type=bottom,waterlogged=false]"))),
        RD_SIDEWALK | RD_STEPS => (b("stone_bricks"), Some(b("stone_brick_slab[type=bottom,waterlogged=false]"))),
        RD_PEDESTRIAN => (b("polished_andesite"), Some(b("polished_andesite_slab[type=bottom,waterlogged=false]"))),
        RD_TRAIL => (b("dirt_path"), Some(b("granite_slab[type=bottom,waterlogged=false]"))),
        RD_TRACK => (b("coarse_dirt"), Some(b("cobblestone_slab[type=bottom,waterlogged=false]"))),
        _ => (b("andesite"), Some(b("andesite_slab[type=bottom,waterlogged=false]"))),
    }
}

/// Road surface code for an OSM highway class (osm::ROAD_CLASSES index + 1).
pub fn road_code(class: u16, text: &str) -> u8 {
    let tag = |k: &str| text.split(';').find_map(|kv| kv.split_once('=').filter(|(kk, _)| *kk == k).map(|(_, v)| v));
    match class {
        1 | 2 => RD_MOTORWAY,
        // JA tagging: trunk = 一般国道, primary = 主要地方道, secondary = 都道府県道
        3 | 4 => RD_NATIONAL,
        5..=8 => RD_PREF,
        9..=12 | 16 | 23 => RD_GENERAL,
        13 | 14 => RD_NARROW,
        15 => RD_TRACK,
        17 => RD_PEDESTRIAN,
        18 | 20 => {
            if matches!(tag("footway"), Some("sidewalk" | "crossing")) {
                RD_SIDEWALK
            } else if matches!(tag("surface"), Some("unpaved" | "dirt" | "ground" | "gravel" | "fine_gravel" | "earth" | "grass" | "rock" | "pebblestone")) {
                RD_TRAIL
            } else {
                RD_PEDESTRIAN
            }
        }
        19 | 22 => {
            if matches!(tag("surface"), Some("paved" | "asphalt" | "concrete" | "paving_stones")) { RD_PEDESTRIAN } else { RD_TRAIL }
        }
        21 => RD_STEPS,
        _ => RD_GENERAL,
    }
}

/// Default full width (m) of a road class when OSM has no width/lanes.
pub fn road_width(class: u16, nums: [f32; 4], text: &str) -> f32 {
    if nums[0] > 0.5 {
        return nums[0];
    }
    let lanes = nums[1];
    let base = match class {
        1 => 7.5,          // one carriageway of a motorway (OSM maps each direction)
        2 => 5.0,
        3 => 8.0,
        4 | 6 | 8 | 10 => 4.5,
        5 => 9.0,
        7 => 7.0,
        9 => 6.0,
        11 | 12 | 16 => 4.5,
        13 => 4.0,
        14 => 3.5,
        15 => 3.0,
        17 => 4.0,
        18 | 20 => if text.contains("footway=sidewalk") { 2.0 } else { 1.8 },
        19 | 22 => 1.2,
        21 => 2.0,
        _ => 4.0,
    };
    if lanes >= 1.0 && class <= 12 { (lanes * 3.25 + 1.5).max(base) } else { base }
}

// ground rail codes (rail_*.tif)
pub const RAIL_TRACK: u8 = 1;
pub const RAIL_BALLAST: u8 = 2;
pub const RAIL_SLAB: u8 = 3;
pub const RAIL_PLATFORM: u8 = 4;

/// Set a column surface to a (possibly half-block) height.
/// `surf` = walking surface in world Y units (continuous). Returns the slab Y if any.
fn set_surface(c: &mut Col, ov: &mut Overlay, i: usize, surf: f32, full: BlockId, slab: Option<BlockId>, ids: &Ids) -> Option<i32> {
    let q = (surf * 2.0).round() / 2.0;
    let half = q.fract().abs() > 0.25 && slab.is_some();
    let top_full = if half { q.floor() as i32 - 1 } else { q.round() as i32 - 1 };
    c.g = top_full;
    c.w = top_full;
    c.top = full;
    c.sub = ids.stone;
    c.sub_n = 2;
    c.deep_n = 0;
    c.flags &= !CF_WATER;
    if half {
        ov.put(i, top_full + 1, slab.unwrap());
        Some(top_full + 1)
    } else {
        None
    }
}

/// Rail block shape from the directions of the connected neighbours.
fn rail_shape(n: bool, s: bool, e: bool, w: bool, up: Option<char>) -> String {
    if let Some(d) = up {
        return format!(
            "ascending_{}",
            match d {
                'n' => "north",
                's' => "south",
                'e' => "east",
                _ => "west",
            }
        );
    }
    match (n, s, e, w) {
        (true, false, true, false) => "north_east".into(),
        (true, false, false, true) => "north_west".into(),
        (false, true, true, false) => "south_east".into(),
        (false, true, false, true) => "south_west".into(),
        (false, false, true, _) | (false, false, _, true) => "east_west".into(),
        (_, _, true, true) if !(n && s) => "east_west".into(),
        _ => "north_south".into(),
    }
}

/// Bridge decks over each window column (for the buildings drawn after them).
pub struct Decks {
    /// Y of the lowest deck underside above the column (i32::MAX: none)
    pub bottom: Vec<i32>,
    /// a railway deck passes over the column
    pub rail: Vec<bool>,
}

/// Returns the decks over each column, so buildings stay under viaducts and
/// station roofs are told from buildings under a road.
pub fn apply(g: &Gen, cols: &mut [Col], ov: &mut Overlay, structs: &[VFeat]) -> Decks {
    let ids = Ids::get();
    let n = g.n;
    let fx = g.fx;
    let concrete = ids.light_gray_concrete;
    // ---- water structures (breakwaters, piers, dams, weirs)
    for i in 0..n * n {
        let s = fx.strct[i];
        if s == 0 || cols[i].out {
            continue;
        }
        let c = &mut cols[i];
        match s {
            1 | 2 => {
                // above the water (sea or lake), low land raised to the same
                let lift = if s == 1 { 3 } else { 2 };
                let top = if c.flags & CF_WATER != 0 { c.w + lift } else { SEA_Y + lift };
                if c.g < top {
                    c.g = top;
                    c.w = top;
                }
                c.top = concrete;
                c.sub = ids.stone;
                c.sub_n = 2;
                c.flags = (c.flags & !CF_WATER) | CF_STRUCT | CF_NO_TREE | CF_NO_PLANT;
            }
            3 | 5 => {
                c.top = concrete;
                c.flags |= CF_STRUCT | CF_NO_TREE | CF_NO_PLANT;
            }
            4 => {
                if c.w > c.g {
                    c.g = c.w;
                }
                c.top = ids.smooth_stone;
                c.flags = (c.flags & !CF_WATER) | CF_STRUCT | CF_NO_TREE | CF_NO_PLANT;
            }
            _ => {}
        }
    }
    // ---- ground roads
    let mut track: HashMap<usize, Vec<i32>> = HashMap::new(); // pixel -> rail block Y(s)
    for i in 0..n * n {
        let code = fx.road[i];
        if code == 0 || cols[i].out || !fx.roadh[i].is_finite() || cols[i].flags & CF_BLDG != 0 || fx.bldg[i] != 0 {
            continue;
        }
        let surf = SEA_Y as f32 + fx.roadh[i] + 1.0;
        let (full, slab) = road_blocks(code);
        let mut c = cols[i];
        let slab_y = set_surface(&mut c, ov, i, surf, full, slab, ids);
        c.flags |= CF_ROAD | CF_NO_TREE | CF_NO_PLANT;
        if code == RD_MEDIAN && slab_y.is_none() {
            // low hedge on the median
            let (x, z) = (g.ox + (i % n) as i64, g.oz + (i / n) as i64);
            if crate::rgen::hash3(x, z, 77) % 3 != 0 {
                ov.put(i, c.g + 1, b("azalea_leaves[distance=1,persistent=true,waterlogged=false]"));
            }
        }
        cols[i] = c;
    }
    // ---- ground rails and platforms
    for i in 0..n * n {
        let code = fx.rail[i];
        if code == 0 || cols[i].out || !fx.railh[i].is_finite() {
            continue;
        }
        let yb = SEA_Y + fx.railh[i].round() as i32;
        let c = &mut cols[i];
        match code {
            RAIL_TRACK | RAIL_BALLAST | RAIL_SLAB => {
                c.g = yb;
                c.w = yb;
                c.top = if code == RAIL_SLAB { ids.smooth_stone } else { ids.gravel };
                c.sub = ids.gravel;
                c.sub_n = 2;
                c.flags = (c.flags & !CF_WATER) | CF_RAIL | CF_NO_TREE | CF_NO_PLANT;
                if code == RAIL_TRACK {
                    track.entry(i).or_default().push(yb + 1);
                }
            }
            RAIL_PLATFORM => {
                if yb + 1 > c.top_y() + 2 {
                    // platform of an elevated station: a slab on its own supports
                    ov.put(i, yb + 1, ids.smooth_stone);
                    ov.put(i, yb, concrete);
                    let (x, z) = (g.ox + (i % n) as i64, g.oz + (i / n) as i64);
                    if x.rem_euclid(8) == 0 && z.rem_euclid(8) == 0 {
                        for y in c.top_y() + 1..yb {
                            ov.put(i, y, concrete);
                        }
                    }
                    c.flags |= CF_RAIL | CF_NO_TREE | CF_NO_PLANT;
                } else {
                    c.g = yb + 1;
                    c.w = yb + 1;
                    c.top = ids.smooth_stone;
                    c.sub = ids.stone;
                    c.sub_n = 2;
                    c.flags = (c.flags & !CF_WATER) | CF_RAIL | CF_NO_TREE | CF_NO_PLANT;
                }
            }
            _ => {}
        }
    }
    // ---- bridges, elevated embankments and tunnels
    let decks = structures(g, cols, ov, structs, &mut track);
    // platform edges next to the tracks get the yellow tactile strip
    for i in 0..n * n {
        if fx.rail[i] != RAIL_PLATFORM || cols[i].out {
            continue;
        }
        let (x, z) = (i % n, i / n);
        let yb = SEA_Y + fx.railh[i].round() as i32;
        // a track beside it on the ground, or on the viaduct at the platform level
        let near_track = [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)].iter().any(|&(dx, dz)| {
            let (xx, zz) = (x as i64 + dx, z as i64 + dz);
            if xx < 0 || zz < 0 || (xx as usize) >= n || (zz as usize) >= n {
                return false;
            }
            let j = zz as usize * n + xx as usize;
            matches!(fx.rail[j], RAIL_TRACK | RAIL_BALLAST | RAIL_SLAB)
                || (fx.rail[j] != RAIL_PLATFORM && decks.rail[j] && (decks.bottom[j] - yb).abs() <= 3)
                || track.get(&j).is_some_and(|v| v.iter().any(|&y| (y - (yb + 1)).abs() <= 2))
        });
        let elevated = ov.get(i, yb + 1) == Some(ids.smooth_stone);
        if near_track {
            if elevated {
                ov.put(i, yb + 1, ids.yellow_concrete);
            } else {
                cols[i].top = ids.yellow_concrete;
            }
        } else {
            // canopy over the platform, on pillars along its middle
            let interior = [(-2i64, 0i64), (2, 0), (0, -2), (0, 2)].iter().all(|&(dx, dz)| {
                let (xx, zz) = (x as i64 + dx, z as i64 + dz);
                xx >= 0 && zz >= 0 && (xx as usize) < n && (zz as usize) < n && fx.rail[zz as usize * n + xx as usize] == RAIL_PLATFORM
            });
            let (wx, wz) = (g.ox + x as i64, g.oz + z as i64);
            if interior && (wx + wz).rem_euclid(10) == 0 {
                for y in yb + 2..yb + 6 {
                    ov.put(i, y, b("light_gray_concrete"));
                }
            }
        }
        ov.put(i, yb + 6, b("smooth_stone_slab[type=bottom,waterlogged=false]"));
    }
    // ---- rail blocks with connected shapes
    // (stacked tracks: a neighbour links at the level within one block)
    let get = |x: i64, z: i64, y: i32| -> Option<i32> {
        if x < 0 || z < 0 || x >= n as i64 || z >= n as i64 {
            return None;
        }
        track.get(&((z as usize) * n + x as usize))?.iter().copied().filter(|&yy| (yy - y).abs() <= 1).min_by_key(|&yy| ((yy - y).abs(), yy))
    };
    let mut rails: Vec<(usize, i32, String)> = Vec::new();
    let mut tk: Vec<(usize, i32)> = track.iter().flat_map(|(&i, v)| v.iter().map(move |&y| (i, y))).collect();
    tk.sort();
    for (i, y) in tk {
        let (x, z) = ((i % n) as i64, (i / n) as i64);
        let nb = |dx: i64, dz: i64| get(x + dx, z + dz, y);
        let (nn, ss, ee, ww) = (nb(0, -1), nb(0, 1), nb(1, 0), nb(-1, 0));
        let up = if nn == Some(y + 1) && ss.is_some_and(|v| v <= y) {
            Some('n')
        } else if ss == Some(y + 1) && nn.is_some_and(|v| v <= y) {
            Some('s')
        } else if ee == Some(y + 1) && ww.is_some_and(|v| v <= y) {
            Some('e')
        } else if ww == Some(y + 1) && ee.is_some_and(|v| v <= y) {
            Some('w')
        } else {
            None
        };
        let shape = rail_shape(nn.is_some(), ss.is_some(), ee.is_some(), ww.is_some(), up);
        rails.push((i, y, shape));
    }
    for (i, y, shape) in rails {
        ov.put(i, y, b(&format!("rail[shape={shape},waterlogged=false]")));
        // the column itself is railway only where the track lies on the ground
        if y <= cols[i].top_y() + 2 {
            cols[i].flags |= CF_RAIL | CF_NO_TREE | CF_NO_PLANT;
        }
    }
    decks
}

const M_BRIDGE: u8 = 1;
const M_EMBANK: u8 = 2;
const M_TUNNEL: u8 = 3;

/// A profiled structure way (see network.rs) prepared for drawing.
struct SWay<'a> {
    pts: &'a [[f64; 2]],
    zs: &'a [f32],
    rail: bool,
    mode: u8,
    hw: f32,
    tracks: i32,
    spacing: f64,
    full: BlockId,
    slab: Option<BlockId>,
}

/// One deck (or tunnel floor) over a pixel: the ways overlapping there at
/// about the same height are merged into one surface.
#[derive(Clone, Copy)]
struct Deck {
    h: f32, // surface height (m)
    way: u32,
    d: f32,   // distance from the centre line of `way`
    axis: u8, // 0: `way` runs mostly along X, 1: along Z
    up: bool,
    next: u32, // next deck over the same pixel
}

/// Bridges, elevated embankments and tunnels. Parallel ways (carriageways,
/// sidewalks, single tracks) that overlap at about the same height form one
/// deck: parapets only on its outer edge, piers on a grid shared by the
/// parallel ways. Returns the lowest deck underside per column.
fn structures(g: &Gen, cols: &mut [Col], ov: &mut Overlay, structs: &[VFeat], track: &mut HashMap<usize, Vec<i32>>) -> Decks {
    let ids = Ids::get();
    let n = g.n;
    let concrete = ids.light_gray_concrete;
    let (x0, z0) = (g.ox as f64, g.oz as f64);
    let mut deck_bottom = vec![i32::MAX; n * n];
    let mut deck_rail = vec![false; n * n];
    let ways: Vec<SWay> = structs
        .iter()
        .filter_map(|f| {
            let pts = f.parts.first()?;
            let zs = f.zs.first()?;
            if pts.len() < 2 || zs.len() != pts.len() {
                return None;
            }
            let mode = if f.flags & F_TUNNEL != 0 {
                M_TUNNEL
            } else if f.flags & F_EMBANKMENT != 0 {
                M_EMBANK
            } else if f.flags & F_BRIDGE != 0 {
                M_BRIDGE
            } else {
                return None;
            };
            let rail = f.nums[3] > 0.5;
            let hs = f.flags & F_HIGHSPEED != 0;
            let tracks = if rail { (f.nums[0].round() as i32).clamp(1, 8) } else { 0 };
            let spacing = if hs { 4.3 } else { 4.0 };
            let width = if rail {
                // a little wider than the track, so parallel tracks share one deck
                if hs { 11.0f32.max(tracks as f32 * 5.0) } else { 3.0 + spacing as f32 * (tracks as f32 - 1.0) + 3.0 }
            } else {
                road_width(f.class, f.nums, &f.text) + 1.0
            };
            let (full, slab) = if rail { (if hs { ids.smooth_stone } else { ids.gravel }, None) } else { road_blocks(road_code(f.class, &f.text)) };
            Some(SWay { pts, zs, rail, mode, hw: width / 2.0, tracks, spacing, full, slab })
        })
        .collect();
    // nearest point of each way per pixel: (pixel, way, distance, height, axis)
    let mut cands: Vec<(u32, u32, f32, f32, u8)> = Vec::new();
    for (wi, w) in ways.iter().enumerate() {
        let mut best: HashMap<usize, (f32, f32, u8)> = HashMap::new();
        let hw = w.hw as f64;
        for k in 0..w.pts.len() - 1 {
            let (a, b2) = (w.pts[k], w.pts[k + 1]);
            let (za, zb) = (w.zs[k], w.zs[k + 1]);
            let (ex, ez) = (b2[0] - a[0], b2[1] - a[1]);
            let seg2 = ex * ex + ez * ez;
            let axis = if ex.abs() >= ez.abs() { 0 } else { 1 };
            let (bx0, bx1) = ((a[0].min(b2[0]) - hw - 1.0 - x0).floor() as i64, (a[0].max(b2[0]) + hw + 1.0 - x0).ceil() as i64);
            let (bz0, bz1) = ((a[1].min(b2[1]) - hw - 1.0 - z0).floor() as i64, (a[1].max(b2[1]) + hw + 1.0 - z0).ceil() as i64);
            for pz in bz0.max(0)..=bz1.min(n as i64 - 1) {
                for px in bx0.max(0)..=bx1.min(n as i64 - 1) {
                    let (cx, cz) = (x0 + px as f64 + 0.5, z0 + pz as f64 + 0.5);
                    let t = if seg2 > 0.0 { (((cx - a[0]) * ex + (cz - a[1]) * ez) / seg2).clamp(0.0, 1.0) } else { 0.0 };
                    let d = ((cx - a[0] - ex * t).powi(2) + (cz - a[1] - ez * t).powi(2)).sqrt() as f32;
                    if d > w.hw {
                        continue;
                    }
                    let i = pz as usize * n + px as usize;
                    let e = best.entry(i).or_insert((f32::MAX, 0.0, 0));
                    if d < e.0 {
                        *e = (d, za + (zb - za) * t as f32, axis);
                    }
                }
            }
        }
        for (i, (d, h, axis)) in best {
            cands.push((i as u32, wi as u32, d, h, axis));
        }
    }
    let up_of = |wi: u32| ways[wi as usize].mode != M_TUNNEL;
    cands.sort_by(|a, b| a.0.cmp(&b.0).then(up_of(a.1).cmp(&up_of(b.1))).then(a.3.total_cmp(&b.3)).then(a.1.cmp(&b.1)));
    // merge the ways over each pixel into decks: same kind (up / tunnel),
    // heights within 2 m; the way whose centre line is nearest gives the height
    let mut first = vec![u32::MAX; n * n];
    let mut decks: Vec<Deck> = Vec::new();
    let mut i = 0;
    while i < cands.len() {
        let p = cands[i].0 as usize;
        let mut j = i;
        while j < cands.len() && cands[j].0 as usize == p {
            j += 1;
        }
        let mut k = i;
        while k < j {
            let up = up_of(cands[k].1);
            let h0 = cands[k].3;
            let (mut m, mut rep) = (k, k);
            while m < j && up_of(cands[m].1) == up && cands[m].3 - h0 <= 2.0 {
                if cands[m].2 < cands[rep].2 {
                    rep = m;
                }
                m += 1;
            }
            let c = cands[rep];
            decks.push(Deck { h: c.3, way: c.1, d: c.2, axis: c.4, up, next: first[p] });
            first[p] = (decks.len() - 1) as u32;
            k = m;
        }
        i = j;
    }
    // parallel ways solved apart differ by a little: even the surface out
    // across neighbouring decks of the same level (twice over ±3 px)
    for _ in 0..2 {
        let mut hs: Vec<f32> = decks.iter().map(|d| d.h).collect();
        for p in 0..n * n {
            let mut di = first[p];
            while di != u32::MAX {
                let d = decks[di as usize];
                let (x, z) = ((p % n) as i64, (p / n) as i64);
                let (mut s, mut c) = (0f32, 0u32);
                for zz in (z - 3).max(0)..=(z + 3).min(n as i64 - 1) {
                    for xx in (x - 3).max(0)..=(x + 3).min(n as i64 - 1) {
                        let mut dj = first[zz as usize * n + xx as usize];
                        while dj != u32::MAX {
                            let e = &decks[dj as usize];
                            if e.up == d.up && (e.h - d.h).abs() <= 2.0 {
                                s += e.h;
                                c += 1;
                            }
                            dj = e.next;
                        }
                    }
                }
                hs[di as usize] = s / c as f32;
                di = d.next;
            }
        }
        for (d, h) in decks.iter_mut().zip(hs) {
            d.h = h;
        }
    }
    // column tops before anything is changed (abutments, parapet tests)
    let top0: Vec<i32> = cols.iter().map(|c| if c.out { i32::MIN } else { c.top_y() }).collect();
    let same_level = |q: usize, h: f32, up: bool| -> bool {
        let mut di = first[q];
        while di != u32::MAX {
            let d = &decks[di as usize];
            if d.up == up && (d.h - h).abs() <= 1.2 {
                return true;
            }
            di = d.next;
        }
        false
    };
    // an outer edge: a neighbour without a deck at this level and (for decks)
    // without ground reaching up to it
    let edge = |p: usize, dk: &Deck| -> bool {
        let (x, z) = ((p % n) as i64, (p / n) as i64);
        let deck = SEA_Y + dk.h.round() as i32;
        [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)].iter().any(|&(dx, dz)| {
            let (xx, zz) = (x + dx, z + dz);
            if xx < 0 || zz < 0 || xx >= n as i64 || zz >= n as i64 {
                return false;
            }
            let q = zz as usize * n + xx as usize;
            // a platform at the level of the deck is open to it
            let platform = g.fx.rail[q] == RAIL_PLATFORM && g.fx.railh[q].is_finite() && (SEA_Y + g.fx.railh[q].round() as i32 - deck).abs() <= 2;
            !same_level(q, dk.h, dk.up) && !platform && (!dk.up || top0[q] < deck)
        })
    };
    let lining = b("stone_bricks");
    let mut deck_y = vec![i32::MIN; decks.len()]; // Y of the block rails stand on
    for p in 0..n * n {
        if first[p] == u32::MAX || cols[p].out {
            continue;
        }
        let mut list: Vec<u32> = Vec::new();
        let mut di = first[p];
        while di != u32::MAX {
            list.push(di);
            di = decks[di as usize].next;
        }
        list.sort_by(|a, b| decks[*a as usize].h.total_cmp(&decks[*b as usize].h));
        let mut lowest_up = true;
        for di in list {
            let dk = decks[di as usize];
            let w = &ways[dk.way as usize];
            let surf = SEA_Y as f32 + dk.h + 1.0; // walking surface (continuous Y)
            let deck = SEA_Y + dk.h.round() as i32; // block whose top is the rail bed
            let slab = if w.rail { None } else { w.slab };
            if dk.up {
                let first_up = lowest_up;
                lowest_up = false;
                if w.mode == M_EMBANK || (first_up && deck <= cols[p].top_y() + 1) {
                    // an embankment, or where the deck meets the ground: a raised ground surface
                    if !first_up && deck <= cols[p].top_y() {
                        continue;
                    }
                    let mut c = cols[p];
                    let sy = set_surface(&mut c, ov, p, surf, w.full, slab, ids);
                    c.flags |= if w.rail { CF_RAIL } else { CF_ROAD } | CF_NO_TREE | CF_NO_PLANT;
                    if w.mode == M_EMBANK && edge(p, &dk) {
                        ov.put(p, sy.unwrap_or(c.g) + 1, concrete);
                    }
                    cols[p] = c;
                    deck_y[di as usize] = c.g + 1;
                    continue;
                }
                cols[p].flags |= CF_NO_TREE;
                // road decks are quantized to half blocks (walkable grades); rail beds stay full blocks
                let (top_y, walk_y) = match slab {
                    Some(sb) => {
                        let q = (surf * 2.0).round() / 2.0;
                        if q.fract().abs() > 0.25 {
                            let y = q.floor() as i32;
                            ov.put(p, y, sb);
                            (y, y + 1)
                        } else {
                            let y = q.round() as i32 - 1;
                            ov.put(p, y, w.full);
                            (y, y + 1)
                        }
                    }
                    None => {
                        ov.put(p, deck, w.full);
                        (deck, deck + 1)
                    }
                };
                // girder: one block, two on high viaducts
                let girder = if top_y - cols[p].top_y() >= 8 { 2 } else { 1 };
                for k in 1..=girder {
                    if ov.get(p, top_y - k).is_none() {
                        ov.put(p, top_y - k, concrete);
                    }
                }
                deck_bottom[p] = deck_bottom[p].min(top_y - girder);
                deck_rail[p] |= w.rail;
                if edge(p, &dk) {
                    // parapet on a full block (a slab edge would leave a gap under it)
                    ov.put(p, top_y, concrete);
                    ov.put(p, walk_y, concrete);
                }
                deck_y[di as usize] = deck + 1;
                // piers on a grid shared by parallel ways, standing on the ground (also through water)
                if w.mode == M_BRIDGE && dk.d < 1.0 {
                    let (wx, wz) = (g.ox + (p % n) as i64, g.oz + (p / n) as i64);
                    let along = if dk.axis == 0 { wx } else { wz };
                    let pitch = if w.rail { 20 } else { 30 };
                    if along.rem_euclid(pitch) < 2 {
                        for y in cols[p].g + 1..top_y - girder {
                            if ov.get(p, y).is_none() {
                                ov.put(p, y, concrete);
                            }
                        }
                    }
                }
            } else {
                // tunnel: carve a box inside the terrain, line it
                let c = cols[p];
                if c.flags & CF_WATER != 0 {
                    continue; // under the sea / a river: nothing visible, never a causeway
                }
                if c.top_y() < deck + 2 {
                    // portal / open cut: a ground road
                    let mut cc = c;
                    set_surface(&mut cc, ov, p, surf, w.full, slab, ids);
                    cc.flags |= if w.rail { CF_RAIL } else { CF_ROAD } | CF_NO_TREE | CF_NO_PLANT;
                    cols[p] = cc;
                    deck_y[di as usize] = cc.g + 1;
                    continue;
                }
                let hgt = if w.rail { 7 } else if w.hw >= 3.0 { 6 } else { 4 };
                let floor_y = match slab {
                    Some(sb) => {
                        let q = (surf * 2.0).round() / 2.0;
                        if q.fract().abs() > 0.25 {
                            let y = q.floor() as i32;
                            ov.put(p, y, sb);
                            ov.put(p, y - 1, w.full);
                            y
                        } else {
                            let y = q.round() as i32 - 1;
                            ov.put(p, y, w.full);
                            y
                        }
                    }
                    None => {
                        ov.put(p, deck, w.full);
                        deck
                    }
                };
                let wall = edge(p, &dk);
                // keep the top ground block (and so any water above it) intact
                let mut ceil = floor_y + hgt + 1;
                for y in floor_y + 1..=floor_y + hgt {
                    if y >= c.g {
                        ceil = y;
                        break;
                    }
                    ov.put(p, y, if wall { lining } else { ids.air });
                }
                if ceil <= c.g {
                    ov.put(p, ceil, lining);
                }
                deck_y[di as usize] = deck + 1;
            }
        }
    }
    // rail tracks: 4-connected pixel paths of each track's offset line, on the deck below them
    for w in ways.iter().filter(|w| w.rail) {
        let up = w.mode != M_TUNNEL;
        let local: Vec<[f64; 2]> = w.pts.iter().map(|p| [p[0] - x0, p[1] - z0]).collect();
        let nm = crate::roads::normals(&local);
        for k in 0..w.tracks {
            let off = (k as f64 - (w.tracks as f64 - 1.0) / 2.0) * w.spacing;
            let tp: Vec<[f64; 2]> = local.iter().zip(&nm).map(|(q, (_, nr))| [q[0] + nr[0] * off, q[1] + nr[1] * off]).collect();
            crate::roads::path4(n, &tp, |p, j| {
                let hj = w.zs[j.min(w.zs.len() - 1)];
                let (mut best, mut bd) = (i32::MIN, 2.0f32);
                let mut di = first[p];
                while di != u32::MAX {
                    let d = &decks[di as usize];
                    if d.up == up && (d.h - hj).abs() <= bd && deck_y[di as usize] != i32::MIN {
                        bd = (d.h - hj).abs();
                        best = deck_y[di as usize];
                    }
                    di = d.next;
                }
                if best != i32::MIN {
                    let v = track.entry(p).or_default();
                    if !v.contains(&best) {
                        v.push(best);
                    }
                }
            });
        }
    }
    Decks { bottom: deck_bottom, rail: deck_rail }
}
