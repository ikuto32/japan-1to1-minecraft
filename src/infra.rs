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
    // a surface laid before (a ground road under a bridge end or a tunnel
    // mouth) leaves its slab or hedge one block over its top: it must not
    // float over a lowered surface or cut into a raised one
    let old = c.top_y();
    let (lo, hi) = (top_full.min(old) + 1, top_full.max(old) + 1);
    ov.v[i].retain(|e| e.0 < lo || e.0 > hi);
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
    /// walking surfaces drawn: (window index, 2 × surface Y, S_* kind, road code; 0 for rails)
    pub surf: Vec<(u32, i32, u8, u8)>,
}

// kinds of drawn surfaces (Decks::surf)
pub const S_GROUND: u8 = 1;
pub const S_DECK: u8 = 2;
pub const S_EDGE: u8 = 3; // deck edge under a parapet
pub const S_EMBANK: u8 = 4;
pub const S_TUNNEL: u8 = 5;
pub const S_PORTAL: u8 = 6; // open cut at a tunnel mouth
pub const S_TUNNEL_WALL: u8 = 7; // tunnel floor under the lining
pub const S_RAIL: u8 = 8; // rail blocks (on the ground, a deck or in a tunnel)

/// 2 × the walking surface of a column whose top full block is `g`, with a
/// bottom slab on it or not.
#[inline]
fn surf2(g: i32, slab: bool) -> i32 {
    2 * g + if slab { 3 } else { 2 }
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
    let mut surfs: Vec<(u32, i32, u8, u8)> = Vec::new();
    let mut track: HashMap<usize, Vec<i32>> = HashMap::new(); // pixel -> rail block Y(s)
    for i in 0..n * n {
        let code = fx.road[i];
        // not inside a building (an open shed is a roof over the road)
        let in_bldg = fx.bldg[i] != 0 && fx.bld[fx.bldg[i] as usize].roof != crate::features::R_OPEN;
        if code == 0 || cols[i].out || !fx.roadh[i].is_finite() || cols[i].flags & CF_BLDG != 0 || in_bldg {
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
        surfs.push((i as u32, surf2(c.g, slab_y.is_some()), S_GROUND, code));
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
    let mut decks = structures(g, cols, ov, structs, &mut track, &mut surfs);
    decks.surf = surfs;
    // the canopy over a platform stands on pillars along its middle line: in
    // every 7 m cell, the middle-line pixel with the lowest hash (pillars
    // stand apart on a platform of any direction, the same in every window)
    let mut pdist = vec![0u8; n * n];
    {
        let mut q = std::collections::VecDeque::new();
        for i in 0..n * n {
            if fx.rail[i] == RAIL_PLATFORM {
                let (x, z) = (i % n, i / n);
                let edge = x == 0 || z == 0 || x + 1 == n || z + 1 == n || [i - 1, i + 1, i - n, i + n].iter().any(|&j| fx.rail[j] != RAIL_PLATFORM);
                if edge {
                    pdist[i] = 1;
                    q.push_back(i);
                }
            }
        }
        while let Some(i) = q.pop_front() {
            let (x, z) = (i % n, i / n);
            if pdist[i] >= 20 || x == 0 || z == 0 || x + 1 == n || z + 1 == n {
                continue;
            }
            for j in [i - 1, i + 1, i - n, i + n] {
                if fx.rail[j] == RAIL_PLATFORM && pdist[j] == 0 {
                    pdist[j] = pdist[i] + 1;
                    q.push_back(j);
                }
            }
        }
    }
    let mut pillar_at: HashMap<(i64, i64), (u32, usize)> = HashMap::new();
    for i in 0..n * n {
        let (x, z) = (i % n, i / n);
        if pdist[i] < 3 || x == 0 || z == 0 || x + 1 == n || z + 1 == n {
            continue;
        }
        if [i - 1, i + 1, i - n, i + n].iter().any(|&j| pdist[j] > pdist[i]) {
            continue; // not on the middle line
        }
        let (wx, wz) = (g.ox + x as i64, g.oz + z as i64);
        let h = crate::rgen::hash3(wx, wz, 11);
        let e = pillar_at.entry((wx.div_euclid(7), wz.div_euclid(7))).or_insert((u32::MAX, i));
        if h < e.0 {
            *e = (h, i);
        }
    }
    let pillars: std::collections::HashSet<usize> = pillar_at.values().map(|v| v.1).collect();
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
        } else if pillars.contains(&i) && decks.bottom[i] == i32::MAX {
            // canopy pillar on the platform's middle line (none under a footbridge)
            for y in yb + 2..yb + 6 {
                ov.put(i, y, b("light_gray_concrete"));
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
        decks.surf.push((i as u32, 2 * y, S_RAIL, 0));
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
    /// road surface code (0 for rails)
    code: u8,
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
fn structures(g: &Gen, cols: &mut [Col], ov: &mut Overlay, structs: &[VFeat], track: &mut HashMap<usize, Vec<i32>>, surfs: &mut Vec<(u32, i32, u8, u8)>) -> Decks {
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
            let code = if rail { 0 } else { road_code(f.class, &f.text) };
            let (full, slab) = if rail { (if hs { ids.smooth_stone } else { ids.gravel }, None) } else { road_blocks(code) };
            Some(SWay { pts, zs, rail, mode, hw: width / 2.0, tracks, spacing, full, slab, code })
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
        let mut groups: Vec<(bool, usize)> = Vec::new(); // (up, representative candidate)
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
            // decks cannot be stacked closer than a storey: where two overlap
            // less than 4.5 m apart (ramps diverging side by side, the edges of
            // parallel decks at other heights) the pixel belongs to the deck
            // whose centre line is nearer; only the outer half of a deck gives
            // way (never the middle of a road), and never a railway deck (its
            // track would break)
            let may_drop = |c: usize| {
                let w = &ways[cands[c].1 as usize];
                !w.rail && cands[c].2 >= 0.5 * w.hw
            };
            match groups.last_mut() {
                Some(last) if up && last.0 && cands[rep].3 - cands[last.1].3 < 4.5 => {
                    let nearer = cands[rep].2 < cands[last.1].2;
                    if nearer && may_drop(last.1) {
                        last.1 = rep;
                    } else if nearer || !may_drop(rep) {
                        groups.push((up, rep));
                    }
                }
                _ => groups.push((up, rep)),
            }
            k = m;
        }
        for (up, rep) in groups {
            let c = cands[rep];
            decks.push(Deck { h: c.3, way: c.1, d: c.2, axis: c.4, up, next: first[p] });
            first[p] = (decks.len() - 1) as u32;
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
    // a tunnel needs its height and a block of ground over its lining: where the
    // terrain is lower (the approach to a portal) the way runs in an open cut
    // instead of a tunnel too low to pass; under another road, a railway or a
    // building only a portal at the surface is cut
    let tunnel_h = |w: &SWay| -> i32 { if w.rail { 7 } else if w.hw >= 3.0 { 6 } else { 4 } };
    let mut tcut = vec![false; decks.len()];
    for p in 0..n * n {
        let c = &cols[p];
        let mut di = first[p];
        while di != u32::MAX {
            let d = &decks[di as usize];
            if !d.up && !c.out && c.flags & CF_WATER == 0 {
                let deck = SEA_Y + d.h.round() as i32;
                let natural = c.flags & (CF_ROAD | CF_RAIL | CF_BLDG | CF_STRUCT) == 0 && g.fx.bldg[p] == 0;
                let top = c.top_y();
                tcut[di as usize] = top < deck + 2 || (natural && top < deck + tunnel_h(&ways[d.way as usize]) + 2);
            }
            di = d.next;
        }
    }
    let cut_at = |q: usize, h: f32| -> bool {
        let mut di = first[q];
        while di != u32::MAX {
            let d = &decks[di as usize];
            if !d.up && (d.h - h).abs() <= 1.2 && tcut[di as usize] {
                return true;
            }
            di = d.next;
        }
        false
    };
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
                // the end of a bridge within three blocks of a ground road under it
                // raises that road to it rather than lying on it: a road bridge over
                // a sidewalk or a lane beside it, the stairs of a footbridge over the
                // sidewalk they land on (a footbridge over a carriageway, a railway
                // over a road cross them)
                let car = |c: u8| matches!(c, RD_MOTORWAY | RD_NATIONAL | RD_PREF | RD_GENERAL | RD_NARROW | RD_TRACK);
                let ground_code = g.fx.road[p];
                let low_over_road = !w.rail
                    && cols[p].flags & CF_ROAD != 0
                    && deck <= cols[p].top_y() + 3
                    && (car(w.code) || !(car(ground_code) || matches!(ground_code, RD_MARK_WHITE | RD_MARK_YELLOW | RD_MEDIAN)));
                if w.mode == M_EMBANK || (first_up && (deck <= cols[p].top_y() + 1 || low_over_road)) {
                    // an embankment, or where the deck meets the ground: a raised ground surface
                    if !first_up && deck <= cols[p].top_y() {
                        continue;
                    }
                    let mut c = cols[p];
                    let sy = set_surface(&mut c, ov, p, surf, w.full, slab, ids);
                    c.flags |= if w.rail { CF_RAIL } else { CF_ROAD } | CF_NO_TREE | CF_NO_PLANT;
                    let wall = w.mode == M_EMBANK && edge(p, &dk);
                    if wall {
                        ov.put(p, sy.unwrap_or(c.g) + 1, concrete);
                    }
                    if !w.rail {
                        surfs.push((p as u32, surf2(c.g, sy.is_some()), if wall { S_EDGE } else if w.mode == M_EMBANK { S_EMBANK } else { S_DECK }, w.code));
                    }
                    cols[p] = c;
                    deck_y[di as usize] = c.g + 1;
                    continue;
                }
                cols[p].flags |= CF_NO_TREE;
                // road decks are quantized to half blocks (walkable grades); rail beds stay full blocks
                let (top_y, walk_y, y2) = match slab {
                    Some(sb) => {
                        let q = (surf * 2.0).round() / 2.0;
                        if q.fract().abs() > 0.25 {
                            let y = q.floor() as i32;
                            ov.put(p, y, sb);
                            (y, y + 1, 2 * y + 1)
                        } else {
                            let y = q.round() as i32 - 1;
                            ov.put(p, y, w.full);
                            (y, y + 1, 2 * y + 2)
                        }
                    }
                    None => {
                        ov.put(p, deck, w.full);
                        (deck, deck + 1, 2 * deck + 2)
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
                let wall = edge(p, &dk);
                if wall {
                    // parapet on a full block (a slab edge would leave a gap under it)
                    ov.put(p, top_y, concrete);
                    ov.put(p, walk_y, concrete);
                }
                if !w.rail {
                    surfs.push((p as u32, y2, if wall { S_EDGE } else { S_DECK }, w.code));
                }
                deck_y[di as usize] = deck + 1;
                // piers on a grid shared by parallel ways, standing on the ground (also
                // through water); one that would stand on a carriageway under the
                // bridge moves across the deck to a median strip or a sidewalk (the
                // middle of the road where there is none), never onto a railway
                if w.mode == M_BRIDGE && dk.d < 1.0 {
                    let (wx, wz) = (g.ox + (p % n) as i64, g.oz + (p / n) as i64);
                    let along = if dk.axis == 0 { wx } else { wz };
                    let pitch = if w.rail { 20 } else { 30 };
                    if along.rem_euclid(pitch) < 2 {
                        let free = |q: usize| -> bool {
                            let c = &cols[q];
                            !c.out && c.flags & CF_RAIL == 0 && (c.flags & CF_ROAD == 0 || matches!(g.fx.road[q], 0 | RD_MEDIAN | RD_SIDEWALK | RD_PEDESTRIAN))
                        };
                        let (x, z) = ((p % n) as i64, (p / n) as i64);
                        let mut at = None;
                        for k in 0..=(w.hw.floor() as i64) {
                            for s in if k == 0 { [0i64, 0] } else { [k, -k] } {
                                let (xx, zz) = if dk.axis == 0 { (x, z + s) } else { (x + s, z) };
                                if at.is_none() && xx >= 0 && zz >= 0 && xx < n as i64 && zz < n as i64 && free(zz as usize * n + xx as usize) {
                                    at = Some(zz as usize * n + xx as usize);
                                }
                            }
                        }
                        // else along the deck, off a road crossing under it (its own deck there)
                        if at.is_none() {
                            for k in 1..=8i64 {
                                for s in [k, -k] {
                                    let (xx, zz) = if dk.axis == 0 { (x + s, z) } else { (x, z + s) };
                                    if at.is_some() || xx < 0 || zz < 0 || xx >= n as i64 || zz >= n as i64 {
                                        continue;
                                    }
                                    let q = zz as usize * n + xx as usize;
                                    let mut dj = first[q];
                                    let mut on_deck = false;
                                    while dj != u32::MAX {
                                        let e = &decks[dj as usize];
                                        on_deck |= e.up && (e.h - dk.h).abs() <= 2.0;
                                        dj = e.next;
                                    }
                                    if on_deck && free(q) {
                                        at = Some(q);
                                    }
                                }
                            }
                        }
                        let at = at.or_else(|| (cols[p].flags & CF_RAIL == 0).then_some(p));
                        if let Some(q) = at {
                            for y in cols[q].g + 1..top_y - girder {
                                if ov.get(q, y).is_none() {
                                    ov.put(q, y, concrete);
                                }
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
                if tcut[di as usize] {
                    // portal / open cut: a ground road
                    let mut cc = c;
                    let sy = set_surface(&mut cc, ov, p, surf, w.full, slab, ids);
                    cc.flags |= if w.rail { CF_RAIL } else { CF_ROAD } | CF_NO_TREE | CF_NO_PLANT;
                    if !w.rail {
                        surfs.push((p as u32, surf2(cc.g, sy.is_some()), S_PORTAL, w.code));
                    }
                    cols[p] = cc;
                    deck_y[di as usize] = cc.g + 1;
                    continue;
                }
                let hgt = tunnel_h(w);
                let (floor_y, y2) = match slab {
                    Some(sb) => {
                        let q = (surf * 2.0).round() / 2.0;
                        if q.fract().abs() > 0.25 {
                            let y = q.floor() as i32;
                            ov.put(p, y, sb);
                            ov.put(p, y - 1, w.full);
                            (y, 2 * y + 1)
                        } else {
                            let y = q.round() as i32 - 1;
                            ov.put(p, y, w.full);
                            (y, 2 * y + 2)
                        }
                    }
                    None => {
                        ov.put(p, deck, w.full);
                        (deck, 2 * deck + 2)
                    }
                };
                let wall = edge(p, &dk);
                if !w.rail {
                    surfs.push((p as u32, y2, if wall { S_TUNNEL_WALL } else { S_TUNNEL }, w.code));
                }
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
                // the portal: a lined head wall over the mouth, facing the open cut
                let (x, z) = ((p % n) as i64, (p / n) as i64);
                let face = [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)].iter().any(|&(dx, dz)| {
                    let (xx, zz) = (x + dx, z + dz);
                    xx >= 0 && zz >= 0 && xx < n as i64 && zz < n as i64 && cut_at(zz as usize * n + xx as usize, dk.h)
                });
                if face && c.flags & (CF_ROAD | CF_RAIL | CF_BLDG | CF_STRUCT) == 0 {
                    for y in ceil + 1..=(ceil + 3).min(c.g) {
                        ov.put(p, y, lining);
                    }
                }
                deck_y[di as usize] = deck + 1;
            }
        }
    }
    // covered cuttings: where a mapped tunnel runs in an open cut (a cut and
    // cover tunnel, the Gaikan expressway in Nerima: the elevation model has the
    // cutting but not its roof), the roof comes back as a concrete lid level
    // with the bank of the cutting (the lower one, where both are in reach),
    // over the tunnel and across to that bank, where it clears the way by the
    // height of a tunnel; the side of a cutting shared with an open roadway
    // (a half-covered cutting) stays open past the tunnel's own width
    {
        const REACH: i64 = 16;
        let lid = ids.light_gray_concrete;
        let mut lid_at: HashMap<usize, i32> = HashMap::new();
        for p in 0..n * n {
            if cols[p].out || cols[p].flags & CF_WATER != 0 {
                continue;
            }
            let mut di = first[p];
            while di != u32::MAX {
                let dk = decks[di as usize];
                let cut = !dk.up && tcut[di as usize];
                di = dk.next;
                if !cut {
                    continue;
                }
                let w = &ways[dk.way as usize];
                let floor = cols[p].top_y();
                let lowest = floor + tunnel_h(w) + 1; // the lowest block the lid may have
                let (x, z) = ((p % n) as i64, (p / n) as i64);
                let (sx, sz) = if dk.axis == 0 { (0i64, 1i64) } else { (1, 0) };
                let mut bank = [i32::MAX; 2];
                let mut spans: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
                for (side, sgn) in [-1i64, 1].into_iter().enumerate() {
                    for k in 1..=REACH {
                        let (xx, zz) = (x + sx * sgn * k, z + sz * sgn * k);
                        if xx < 0 || zz < 0 || xx >= n as i64 || zz >= n as i64 {
                            break;
                        }
                        let q = zz as usize * n + xx as usize;
                        if cols[q].out {
                            break;
                        }
                        if top0[q] >= lowest {
                            bank[side] = top0[q];
                            break;
                        }
                        spans[side].push(q);
                    }
                }
                let ly = bank[0].min(bank[1]);
                if ly == i32::MAX {
                    continue; // no bank in reach: an open cut among open roadways
                }
                let [s0, s1] = spans;
                let span = std::iter::once(p).chain(if bank[0] != i32::MAX { s0 } else { Vec::new() }).chain(if bank[1] != i32::MAX { s1 } else { Vec::new() });
                for q in span {
                    let e = lid_at.entry(q).or_insert(ly);
                    *e = (*e).min(ly);
                }
            }
        }
        // the first air block over each walking surface drawn so far
        let mut air: HashMap<u32, i32> = HashMap::new();
        for &(i, y2, _, _) in surfs.iter() {
            let e = air.entry(i).or_insert(i32::MIN);
            *e = (*e).max((y2 + 1).div_euclid(2));
        }
        for (q, ly) in lid_at {
            let c = cols[q];
            // not over a road or a railway of its own (one crossing the cutting
            // on the ground), into a building, a bridge deck or the bank itself
            let own = {
                let mut di = first[q];
                let mut f = false;
                while di != u32::MAX {
                    let d = decks[di as usize];
                    f |= !d.up && tcut[di as usize];
                    di = d.next;
                }
                f
            };
            if (!own && c.flags & (CF_ROAD | CF_RAIL) != 0) || c.flags & (CF_BLDG | CF_STRUCT) != 0 && !own || deck_bottom[q] <= ly + 1 || c.top_y() >= ly {
                continue;
            }
            // four blocks of headroom over a road, seven over the tracks
            let low = match air.get(&(q as u32)) {
                Some(&a) => a + 4,
                None if c.flags & CF_RAIL != 0 => c.top_y() + 8,
                None => c.top_y() + 1,
            };
            for y in [ly, ly - 1] {
                if y >= low && ov.get(q, y).is_none() {
                    ov.put(q, y, lid);
                }
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
    Decks { bottom: deck_bottom, rail: deck_rail, surf: Vec::new() }
}
