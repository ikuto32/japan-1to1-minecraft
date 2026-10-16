//! Roads, railways, platforms, bridges, tunnels and water structures.
//!
//! Ground-level roads/rails come from the feature rasters (road/roadh,
//! rail/railh); bridges and tunnels from the profiled OSM ways
//! (`vec_net_struct.bin`). Road surfaces are quantized to half blocks
//! (bottom slabs) so normal grades can be walked without jumping.

use crate::blocks::{BlockId, b};
use crate::osm::{F_BRIDGE, F_TUNNEL};
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

pub fn apply(g: &Gen, cols: &mut [Col], ov: &mut Overlay, structs: &[VFeat]) {
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
                let top = if s == 1 { SEA_Y + 3 } else { SEA_Y + 2 };
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
    let mut track: HashMap<usize, i32> = HashMap::new(); // pixel -> rail block Y
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
                    track.insert(i, yb + 1);
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
    // platform edges next to the tracks get the yellow tactile strip
    for i in 0..n * n {
        if fx.rail[i] != RAIL_PLATFORM || cols[i].out {
            continue;
        }
        let (x, z) = (i % n, i / n);
        let near_track = [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)].iter().any(|&(dx, dz)| {
            let (xx, zz) = (x as i64 + dx, z as i64 + dz);
            xx >= 0 && zz >= 0 && (xx as usize) < n && (zz as usize) < n && matches!(fx.rail[zz as usize * n + xx as usize], RAIL_TRACK | RAIL_BALLAST | RAIL_SLAB)
        });
        let yb = SEA_Y + fx.railh[i].round() as i32;
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
    // ---- bridges and tunnels
    for f in structs {
        let Some(pts) = f.parts.first() else { continue };
        let Some(zs) = f.zs.first() else { continue };
        if pts.len() < 2 || zs.len() != pts.len() {
            continue;
        }
        let rail = f.nums[3] > 0.5;
        let bridge = f.flags & F_BRIDGE != 0;
        let tunnel = f.flags & F_TUNNEL != 0;
        if !bridge && !tunnel {
            continue;
        }
        let hs = f.flags & crate::osm::F_HIGHSPEED != 0;
        let tracks = if rail { (f.nums[0].round() as i32).clamp(1, 8) } else { 0 };
        let spacing = if hs { 4.3 } else { 4.0 };
        let width = if rail {
            if hs { 11.0f32.max(tracks as f32 * 5.0) } else { 3.0 + spacing as f32 * (tracks as f32 - 1.0) + 2.0 }
        } else {
            road_width(f.class, f.nums, &f.text) + 1.0
        };
        let code = if rail { 0 } else { road_code(f.class, &f.text) };
        let (full, slab) = if rail { (if hs { ids.smooth_stone } else { ids.gravel }, None) } else { road_blocks(code) };
        let hw = width / 2.0;
        let (x0, z0) = (g.ox as f64, g.oz as f64);
        // nearest segment per pixel: (dist, height, arc length)
        let mut best: HashMap<usize, (f32, f32, f64)> = HashMap::new();
        let mut arc = 0.0f64;
        for k in 0..pts.len() - 1 {
            let (a, b2) = (pts[k], pts[k + 1]);
            let (za, zb) = (zs[k], zs[k + 1]);
            let seg = ((b2[0] - a[0]).powi(2) + (b2[1] - a[1]).powi(2)).sqrt();
            let (bx0, bx1) = ((a[0].min(b2[0]) - hw as f64 - 1.0 - x0).floor() as i64, (a[0].max(b2[0]) + hw as f64 + 1.0 - x0).ceil() as i64);
            let (bz0, bz1) = ((a[1].min(b2[1]) - hw as f64 - 1.0 - z0).floor() as i64, (a[1].max(b2[1]) + hw as f64 + 1.0 - z0).ceil() as i64);
            for pz in bz0.max(0)..=bz1.min(n as i64 - 1) {
                for px in bx0.max(0)..=bx1.min(n as i64 - 1) {
                    let (cx, cz) = (x0 + px as f64 + 0.5, z0 + pz as f64 + 0.5);
                    let t = if seg > 0.0 { (((cx - a[0]) * (b2[0] - a[0]) + (cz - a[1]) * (b2[1] - a[1])) / (seg * seg)).clamp(0.0, 1.0) } else { 0.0 };
                    let (qx, qz) = (a[0] + (b2[0] - a[0]) * t, a[1] + (b2[1] - a[1]) * t);
                    let d = ((cx - qx).powi(2) + (cz - qz).powi(2)).sqrt() as f32;
                    if d > hw {
                        continue;
                    }
                    let i = pz as usize * n + px as usize;
                    let h = za + (zb - za) * t as f32;
                    let e = best.entry(i).or_insert((f32::MAX, 0.0, 0.0));
                    if d < e.0 {
                        *e = (d, h, arc + seg * t);
                    }
                }
            }
            arc += seg;
        }
        // rail tracks: 4-connected pixel paths of each track's offset line
        let mut track_px: HashMap<usize, f32> = HashMap::new();
        if rail {
            let local: Vec<[f64; 2]> = pts.iter().map(|p| [p[0] - x0, p[1] - z0]).collect();
            let nm = crate::roads::normals(&local);
            for k in 0..tracks {
                let off = (k as f64 - (tracks as f64 - 1.0) / 2.0) * spacing;
                let tp: Vec<[f64; 2]> = local.iter().zip(&nm).map(|(q, (_, nr))| [q[0] + nr[0] * off, q[1] + nr[1] * off]).collect();
                crate::roads::path4(n, &tp, |p, j| {
                    track_px.insert(p, zs[j.min(zs.len() - 1)]);
                });
            }
        }
        for (&i, &(d, h, s)) in &best {
            if cols[i].out {
                continue;
            }
            let edge = d > hw - 1.0;
            let on_track = track_px.contains_key(&i);
            let surf = SEA_Y as f32 + h + 1.0; // walking surface (continuous Y)
            let deck = SEA_Y + h.round() as i32; // block whose top is the rail bed
            if bridge {
                let c = &mut cols[i];
                if deck <= c.top_y() + 1 {
                    // the deck meets the ground (abutment): treat like a ground road
                    let mut cc = *c;
                    set_surface(&mut cc, ov, i, surf, full, slab, ids);
                    cc.flags |= if rail { CF_RAIL } else { CF_ROAD } | CF_NO_TREE | CF_NO_PLANT;
                    *c = cc;
                    if on_track {
                        track.insert(i, cc.g + 1);
                    }
                    continue;
                }
                c.flags |= CF_NO_TREE;
                // road decks are quantized to half blocks (walkable grades); rail beds stay full blocks
                let (top_y, walk_y) = match slab {
                    Some(sb) if !rail => {
                        let q = (surf * 2.0).round() / 2.0;
                        if q.fract().abs() > 0.25 {
                            let y = q.floor() as i32;
                            ov.put(i, y, sb);
                            ov.put(i, y - 1, concrete);
                            (y, y + 1)
                        } else {
                            let y = q.round() as i32 - 1;
                            ov.put(i, y, full);
                            ov.put(i, y - 1, concrete);
                            (y, y + 1)
                        }
                    }
                    _ => {
                        ov.put(i, deck, full);
                        ov.put(i, deck - 1, concrete);
                        (deck, deck + 1)
                    }
                };
                if edge {
                    // parapet on a full block (a slab edge would leave a gap under it)
                    ov.put(i, top_y, concrete);
                    ov.put(i, walk_y, concrete);
                }
                if on_track {
                    track.insert(i, deck + 1);
                }
                // piers every ~30 m, standing on the ground (also through water)
                if d < 1.0 && (s.rem_euclid(30.0)) < 2.0 {
                    for y in c.g + 1..top_y - 1 {
                        ov.put(i, y, concrete);
                    }
                }
            } else {
                // tunnel: carve a box inside the terrain, line it
                let hgt = if rail { 7 } else { 6 };
                let c = cols[i];
                if c.flags & CF_WATER != 0 {
                    continue; // under the sea / a river: nothing visible, never a causeway
                }
                if c.top_y() < deck + 2 {
                    // portal / open cut: a ground road
                    let mut cc = c;
                    set_surface(&mut cc, ov, i, surf, full, slab, ids);
                    cc.flags |= if rail { CF_RAIL } else { CF_ROAD } | CF_NO_TREE | CF_NO_PLANT;
                    cols[i] = cc;
                    if on_track {
                        track.insert(i, cc.g + 1);
                    }
                    continue;
                }
                let lining = b("stone_bricks");
                // floor: half-block steps for roads, full blocks for the rail bed
                let floor_y = match slab {
                    Some(sb) if !rail => {
                        let q = (surf * 2.0).round() / 2.0;
                        if q.fract().abs() > 0.25 {
                            let y = q.floor() as i32;
                            ov.put(i, y, sb);
                            ov.put(i, y - 1, full);
                            y
                        } else {
                            let y = q.round() as i32 - 1;
                            ov.put(i, y, full);
                            y
                        }
                    }
                    _ => {
                        ov.put(i, deck, full);
                        deck
                    }
                };
                // keep the top ground block (and so any water above it) intact
                let mut ceil = floor_y + hgt + 1;
                for y in floor_y + 1..=floor_y + hgt {
                    if y >= c.g {
                        ceil = y;
                        break;
                    }
                    ov.put(i, y, if edge { lining } else { ids.air });
                }
                if ceil <= c.g {
                    ov.put(i, ceil, lining);
                }
                if on_track {
                    track.insert(i, deck + 1);
                }
            }
        }
    }
    // ---- rail blocks with connected shapes
    let get = |x: i64, z: i64| -> Option<i32> {
        if x < 0 || z < 0 || x >= n as i64 || z >= n as i64 {
            return None;
        }
        track.get(&((z as usize) * n + x as usize)).copied()
    };
    let mut rails: Vec<(usize, i32, String)> = Vec::new();
    for (&i, &y) in &track {
        let (x, z) = ((i % n) as i64, (i / n) as i64);
        let nb = |dx: i64, dz: i64| get(x + dx, z + dz).filter(|&yy| (yy - y).abs() <= 1);
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
        cols[i].flags |= CF_RAIL | CF_NO_TREE | CF_NO_PLANT;
    }
}
