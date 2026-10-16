//! Verification helpers: strip light from region files (so the vanilla server
//! relights them) and compare sky light between two region files.

use crate::nbt::{self, Tag};
use crate::region;
use anyhow::{Context, Result};
use std::path::Path;

fn chunks_of(path: &Path) -> Result<Vec<(usize, Tag)>> {
    let mut v = Vec::new();
    for i in 0..1024 {
        if let Some(raw) = region::read_chunk(path, i % 32, i / 32)? {
            v.push((i, nbt::read_root(&raw)?.1));
        }
    }
    Ok(v)
}

/// Copy a region file with isLightOn = 0 and all light arrays removed
/// (and, with `heightmaps`, the Heightmaps too) so the server recomputes them.
pub fn strip_light(src: &Path, dst: &Path, heightmaps: bool) -> Result<usize> {
    let mut out: Vec<Option<Vec<u8>>> = vec![None; 1024];
    let mut n = 0;
    for (i, mut t) in chunks_of(src)? {
        t.set("isLightOn", Tag::Byte(0));
        if heightmaps {
            t.remove("Heightmaps");
        }
        if let Some(Tag::List(secs)) = t.get_mut("sections") {
            for s in secs.iter_mut() {
                s.remove("SkyLight");
                s.remove("BlockLight");
            }
        }
        out[i] = Some(region::compress_chunk(&nbt::write_root("", &t), 6));
        n += 1;
    }
    std::fs::write(dst, region::build_region(&out, 0)?)?;
    Ok(n)
}

fn block_names(sec: &Tag) -> Vec<String> {
    let mut names = vec![String::new(); 4096];
    let Some(bs) = sec.get("block_states") else { return names };
    let pal: Vec<String> = match bs.get("palette") {
        Some(Tag::List(p)) => p
            .iter()
            .map(|e| match e.get("Name") {
                Some(Tag::String(s)) => s.clone(),
                _ => String::new(),
            })
            .collect(),
        _ => vec![],
    };
    match bs.get("data") {
        Some(Tag::LongArray(d)) => {
            let bits = ((usize::BITS - (pal.len() - 1).leading_zeros()) as usize).max(4);
            let per = 64 / bits;
            for i in 0..4096 {
                let v = ((d[i / per] as u64) >> ((i % per) * bits)) & ((1u64 << bits) - 1);
                names[i] = pal.get(v as usize).cloned().unwrap_or_default();
            }
        }
        _ => {
            for n in names.iter_mut() {
                *n = pal.first().cloned().unwrap_or_default();
            }
        }
    }
    names
}

/// Effective sky light of every block of a chunk following vanilla's rules for
/// missing sections (look up to the next stored section's bottom row; nothing
/// stored above → 15). Returns (sectionY -> 4096 levels) for sections in range.
fn effective_light(t: &Tag) -> Vec<(i32, Vec<u8>, Vec<String>)> {
    let Some(Tag::List(secs)) = t.get("sections") else { return vec![] };
    let mut stored: Vec<(i32, Option<Vec<u8>>, Vec<String>)> = secs
        .iter()
        .map(|s| {
            let y = match s.get("Y") {
                Some(Tag::Byte(b)) => *b as i32,
                _ => 0,
            };
            let l = match s.get("SkyLight") {
                Some(Tag::ByteArray(a)) => Some(a.iter().map(|x| *x as u8).collect::<Vec<u8>>()),
                _ => None,
            };
            (y, l, block_names(s))
        })
        .collect();
    stored.sort_by_key(|s| s.0);
    let nib = |a: &[u8], i: usize| -> u8 { if i & 1 == 0 { a[i >> 1] & 15 } else { a[i >> 1] >> 4 } };
    let top_stored = stored.iter().rposition(|s| s.1.is_some());
    let mut out = Vec::new();
    for (k, (y, l, names)) in stored.iter().enumerate() {
        let mut v = vec![0u8; 4096];
        match l {
            Some(a) => {
                for i in 0..4096 {
                    v[i] = nib(a, i);
                }
            }
            None => {
                let above = stored[k + 1..].iter().find(|s| s.1.is_some());
                match (above, top_stored) {
                    (Some(s), Some(ts)) if k < ts => {
                        let a = s.1.as_ref().unwrap();
                        for i in 0..4096 {
                            v[i] = nib(a, i % 256);
                        }
                    }
                    _ => v.fill(15),
                }
            }
        }
        out.push((*y, v, names.clone()));
    }
    out
}

/// Compare sky light of two region files (only non-opaque blocks matter for
/// rendering; mismatches inside full solid blocks are reported separately).
pub fn compare_light(a: &Path, b: &Path, verbose: usize) -> Result<()> {
    let ca = chunks_of(a)?;
    let cb = chunks_of(b)?;
    let mut total = 0usize;
    let mut bad_open = 0usize;
    let mut bad_solid = 0usize;
    let mut shown = 0;
    let mut compared_chunks = 0usize;
    for (i, ta) in &ca {
        let Some((_, tb)) = cb.iter().find(|(j, _)| j == i) else { continue };
        // only chunks vanilla relit, with all 8 neighbours relit too
        let lit = |k: i64| -> bool {
            cb.iter().any(|(j, t)| *j as i64 == k && matches!(t.get("isLightOn"), Some(Tag::Byte(1))))
        };
        let (ix, iz) = ((*i % 32) as i64, (*i / 32) as i64);
        let mut ok = true;
        for dz in -1..=1 {
            for dx in -1..=1 {
                let (x, z) = (ix + dx, iz + dz);
                if x < 0 || z < 0 || x >= 32 || z >= 32 || !lit(z * 32 + x) {
                    ok = false;
                }
            }
        }
        if !ok {
            continue;
        }
        compared_chunks += 1;
        let la = effective_light(ta);
        let lb = effective_light(tb);
        let cx = match ta.get("xPos") {
            Some(Tag::Int(v)) => *v,
            _ => 0,
        };
        let cz = match ta.get("zPos") {
            Some(Tag::Int(v)) => *v,
            _ => 0,
        };
        for (ya, va, names) in la.iter() {
            let Some((_, vb, _)) = lb.iter().find(|s| s.0 == *ya) else { continue };
            for idx in 0..4096 {
                total += 1;
                if va[idx] != vb[idx] {
                    let name = &names[idx];
                    let open = name.ends_with(":air") || name.ends_with(":water");
                    if open {
                        bad_open += 1;
                    } else {
                        bad_solid += 1;
                    }
                    if open && shown < verbose {
                        let (x, z, y) = (idx % 16, (idx / 16) % 16, idx / 256);
                        println!(
                            "  diff at X={} Y={} Z={} ({}): ours {} vanilla {}",
                            cx * 16 + x as i32,
                            ya * 16 + y as i32,
                            cz * 16 + z as i32,
                            name,
                            va[idx],
                            vb[idx]
                        );
                        shown += 1;
                    }
                }
            }
        }
    }
    println!("compared {} blocks in {} chunks: {} differ in air/water, {} differ inside solid blocks", total, compared_chunks, bad_open, bad_solid);
    Ok(())
}

/// Compare block states and heightmaps of chunks present in both files.
pub fn compare_blocks(a: &Path, b: &Path) -> Result<()> {
    let ca = chunks_of(a)?;
    let cb = chunks_of(b)?;
    let (mut nchunks, mut bdiff, mut hdiff, mut tick) = (0usize, 0usize, 0usize, 0usize);
    for (i, ta) in &ca {
        let Some((_, tb)) = cb.iter().find(|(j, _)| j == i) else { continue };
        // only chunks the server actually loaded and re-saved
        if !matches!(tb.get("isLightOn"), Some(Tag::Byte(1))) {
            continue;
        }
        nchunks += 1;
        let secs = |t: &Tag| -> Vec<(i32, Vec<String>)> {
            let Some(Tag::List(s)) = t.get("sections") else { return vec![] };
            let mut v: Vec<(i32, Vec<String>)> = s
                .iter()
                .map(|x| {
                    let y = match x.get("Y") {
                        Some(Tag::Byte(b)) => *b as i32,
                        _ => 0,
                    };
                    (y, block_names(x))
                })
                .collect();
            v.sort_by_key(|x| x.0);
            v
        };
        let sb = secs(tb);
        for (sy, na) in secs(ta).iter() {
            let Some((_, nb)) = sb.iter().find(|s| s.0 == *sy) else { continue };
            for (idx, (x, y)) in na.iter().zip(nb.iter()).enumerate() {
                if x != y {
                    if bdiff < 12 || std::env::var("BLOCKCMP_ALL").is_ok() {
                        let (cx, cz) = (match ta.get("xPos") { Some(Tag::Int(v)) => *v, _ => 0 }, match ta.get("zPos") { Some(Tag::Int(v)) => *v, _ => 0 });
                        println!("  X={} Y={} Z={}: ours {} vanilla {}", cx * 16 + (idx % 16) as i32, sy * 16 + (idx / 256) as i32, cz * 16 + ((idx / 16) % 16) as i32, x, y);
                    }
                    bdiff += 1;
                }
            }
        }
        for k in ["MOTION_BLOCKING", "MOTION_BLOCKING_NO_LEAVES", "OCEAN_FLOOR", "WORLD_SURFACE"] {
            let ha = ta.get("Heightmaps").and_then(|h| h.get(k));
            let hb = tb.get("Heightmaps").and_then(|h| h.get(k));
            if ha != hb {
                if hdiff < 8 {
                    let (cx, cz) = (match ta.get("xPos") { Some(Tag::Int(v)) => *v, _ => 0 }, match ta.get("zPos") { Some(Tag::Int(v)) => *v, _ => 0 });
                    let dec = |h: Option<&Tag>| -> Vec<i64> {
                        match h {
                            Some(Tag::LongArray(a)) => (0..256).map(|i| (((a[i / 5] as u64) >> ((i % 5) * 12)) & 0xfff) as i64 - 1 - 2032).collect(),
                            _ => vec![],
                        }
                    };
                    let (va, vb) = (dec(ha), dec(hb));
                    let diffs: Vec<String> = (0..256.min(va.len()).min(vb.len())).filter(|i| va[*i] != vb[*i]).take(4).map(|i| format!("(x{} z{}: ours {} vanilla {})", i % 16, i / 16, va[i], vb[i])).collect();
                    println!("  heightmap {k} differs in chunk {cx},{cz}: {}", if vb.is_empty() { "missing in vanilla".to_string() } else { diffs.join(" ") });
                }
                hdiff += 1;
            }
        }
        for k in ["fluid_ticks", "block_ticks"] {
            if let Some(Tag::List(l)) = tb.get(k) {
                tick += l.len();
            }
        }
    }
    println!("{nchunks} chunks: {bdiff} blocks differ, {hdiff} heightmaps differ, {tick} scheduled ticks in vanilla save");
    Ok(())
}

fn dye(c: &str) -> Option<[u8; 3]> {
    Some(match c {
        "white" => [233, 236, 236],
        "light_gray" => [142, 142, 134],
        "gray" => [62, 68, 71],
        "black" => [20, 21, 25],
        "brown" => [114, 71, 40],
        "red" => [160, 39, 34],
        "orange" => [240, 118, 19],
        "yellow" => [248, 197, 39],
        "lime" => [112, 185, 25],
        "green" => [84, 109, 27],
        "cyan" => [21, 137, 145],
        "light_blue" => [58, 175, 217],
        "blue" => [53, 57, 157],
        "purple" => [121, 42, 172],
        "magenta" => [189, 68, 179],
        "pink" => [237, 141, 172],
        _ => return None,
    })
}

pub fn block_color(name: &str) -> [u8; 3] {
    let n = name.trim_start_matches("minecraft:");
    // slabs / stairs / walls look like their material from above
    let base = n.trim_end_matches("_slab").trim_end_matches("_stairs").trim_end_matches("_wall");
    let base = match base {
        "smooth_stone" | "stone_brick" | "polished_andesite" | "andesite" | "cobblestone" | "granite" | "polished_blackstone" | "polished_deepslate" | "smooth_quartz" | "smooth_sandstone" | "mud_brick" | "deepslate_tile" => base,
        _ => base,
    };
    for (suffix, k) in [("_concrete", 1.0f32), ("_terracotta", 0.75), ("_stained_glass", 1.1), ("_carpet", 1.0), ("_wool", 1.0)] {
        if let Some(c) = base.strip_suffix(suffix).and_then(dye) {
            return c.map(|v| (v as f32 * k).min(255.0) as u8);
        }
    }
    if base.ends_with("_leaves") {
        return match base {
            "spruce_leaves" => [60, 95, 60],
            "birch_leaves" => [110, 150, 70],
            "cherry_leaves" => [235, 170, 200],
            "dark_oak_leaves" => [55, 110, 35],
            "jungle_leaves" => [70, 140, 30],
            "azalea_leaves" => [90, 130, 45],
            _ => [72, 125, 40],
        };
    }
    if base.ends_with("_log") || base.ends_with("_wood") {
        return [100, 75, 50];
    }
    if base.ends_with("_planks") {
        return match base {
            "spruce_planks" => [115, 85, 50],
            "dark_oak_planks" => [65, 45, 25],
            "birch_planks" => [195, 180, 125],
            _ => [160, 130, 80],
        };
    }
    match base {
        "grass_block" | "short_grass" | "tall_grass" | "fern" | "large_fern" => [96, 150, 60],
        "moss_block" => [90, 110, 45],
        "dirt" | "coarse_dirt" | "rooted_dirt" => [120, 90, 60],
        "podzol" => [92, 64, 30],
        "farmland" => [110, 75, 45],
        "mud" | "packed_mud" => [70, 60, 55],
        "dirt_path" => [150, 125, 70],
        "stone" | "stone_bricks" | "stone_brick" | "cobblestone" => [125, 125, 125],
        "smooth_stone" => [160, 160, 160],
        "andesite" | "polished_andesite" => [140, 140, 136],
        "granite" => [150, 105, 85],
        "tuff" => [100, 100, 90],
        "deepslate_tiles" | "deepslate_tile" | "polished_deepslate" => [70, 70, 75],
        "polished_blackstone" | "blackstone" => [45, 40, 48],
        "bricks" | "mud_brick" => [150, 90, 75],
        "gravel" => [150, 140, 135],
        "sand" => [220, 210, 160],
        "sandstone" | "smooth_sandstone" | "cut_sandstone" => [215, 205, 155],
        "smooth_quartz" | "quartz_block" => [235, 230, 225],
        "iron_block" => [220, 220, 220],
        "dark_prismarine" => [50, 90, 80],
        "glass" | "glass_pane" => [175, 210, 220],
        "snow_block" | "snow" => [245, 250, 255],
        "water" => [50, 90, 200],
        "seagrass" => [70, 125, 110],
        "rail" | "powered_rail" => [110, 95, 80],
        "daylight_detector" => [40, 50, 80],
        "bamboo" => [95, 145, 40],
        "wheat" | "carrots" | "potatoes" | "beetroots" => [150, 160, 60],
        "poppy" | "dandelion" | "azure_bluet" | "oxeye_daisy" | "cornflower" => [120, 160, 70],
        "bedrock" => [30, 30, 30],
        "air" | "" => [0, 0, 0],
        _ => [255, 0, 255],
    }
}

/// Top-down render of a set of region files (top block colour, hillshaded),
/// one pixel per `step` blocks.
pub fn render(dir: &Path, rx0: i64, rz0: i64, nx: i64, nz: i64, out: &Path, step: usize) -> Result<()> {
    use rayon::prelude::*;
    let rs = 512 / step; // pixels per region side
    let (w, h) = (nx as usize * rs, nz as usize * rs);
    let regs: Vec<(i64, i64)> = (rz0..rz0 + nz).flat_map(|rz| (rx0..rx0 + nx).map(move |rx| (rx, rz))).collect();
    let tiles: Vec<Option<(Vec<u8>, Vec<i32>)>> = regs
        .par_iter()
        .map(|&(rx, rz)| -> Option<(Vec<u8>, Vec<i32>)> {
            let p = dir.join(format!("r.{rx}.{rz}.mca"));
            if !p.exists() {
                return None;
            }
            let mut rgb = vec![0u8; rs * rs * 3];
            let mut top = vec![i32::MIN; rs * rs];
            for (i, t) in chunks_of(&p).ok()? {
                let Some(Tag::List(secs)) = t.get("sections") else { continue };
                let Some(Tag::LongArray(hm)) = t.get("Heightmaps").and_then(|h| h.get("WORLD_SURFACE")) else { continue };
                let mut by_y: std::collections::HashMap<i32, &Tag> = std::collections::HashMap::new();
                for s in secs {
                    if let Some(Tag::Byte(b)) = s.get("Y") {
                        by_y.insert(*b as i32, s);
                    }
                }
                let block_at = |y: i32, idx: usize| -> String {
                    let Some(s) = by_y.get(&y.div_euclid(16)) else { return String::new() };
                    let Some(bs) = s.get("block_states") else { return String::new() };
                    let pal: Vec<&str> = match bs.get("palette") {
                        Some(Tag::List(p)) => p.iter().map(|e| match e.get("Name") { Some(Tag::String(s)) => s.as_str(), _ => "" }).collect(),
                        _ => vec![],
                    };
                    let k = (y.rem_euclid(16) as usize) * 256 + idx;
                    let v = match bs.get("data") {
                        Some(Tag::LongArray(d)) => {
                            let bits = ((usize::BITS - (pal.len() - 1).leading_zeros()) as usize).max(4);
                            let per = 64 / bits;
                            (((d[k / per] as u64) >> ((k % per) * bits)) & ((1u64 << bits) - 1)) as usize
                        }
                        _ => 0,
                    };
                    pal.get(v).map(|s| s.to_string()).unwrap_or_default()
                };
                let (lx, lz) = (i % 32, i / 32);
                for idx in 0..256 {
                    let (bx, bz) = (lx * 16 + idx % 16, lz * 16 + idx / 16);
                    if bx % step != 0 || bz % step != 0 {
                        continue;
                    }
                    let v = ((hm[idx / 5] as u64) >> ((idx % 5) * 12)) & 0xfff;
                    if v == 0 {
                        continue;
                    }
                    let y = v as i32 - 1 - 2032;
                    let k = (bz / step) * rs + bx / step;
                    top[k] = y;
                    rgb[k * 3..k * 3 + 3].copy_from_slice(&block_color(&block_at(y, idx)));
                }
            }
            Some((rgb, top))
        })
        .collect();
    let mut rgb = vec![0u8; w * h * 3];
    let mut top = vec![i32::MIN; w * h];
    for (&(rx, rz), t) in regs.iter().zip(tiles) {
        let Some((r, tp)) = t else { continue };
        let (ox, oz) = (((rx - rx0) as usize) * rs, ((rz - rz0) as usize) * rs);
        for j in 0..rs {
            let d = (oz + j) * w + ox;
            rgb[d * 3..(d + rs) * 3].copy_from_slice(&r[j * rs * 3..(j + 1) * rs * 3]);
            top[d..d + rs].copy_from_slice(&tp[j * rs..(j + 1) * rs]);
        }
    }
    let k_shade = 0.08 / step as f32;
    for z in 1..h {
        for x in 1..w {
            let k = z * w + x;
            if top[k] == i32::MIN || top[k - w - 1] == i32::MIN {
                continue;
            }
            let s = 1.0 + ((top[k] - top[k - w - 1]) as f32 * k_shade).clamp(-0.45, 0.45);
            for c in 0..3 {
                rgb[k * 3 + c] = (rgb[k * 3 + c] as f32 * s).clamp(0.0, 255.0) as u8;
            }
        }
    }
    crate::raster::write_png(out, w, h, &rgb)
}

/// Print the blocks of one column (X, Z) between y0 and y1 from a region file.
pub fn column(path: &Path, x: i64, z: i64, y0: i32, y1: i32) -> Result<()> {
    let (lx, lz) = (x.div_euclid(16).rem_euclid(32) as usize, z.div_euclid(16).rem_euclid(32) as usize);
    let Some(raw) = region::read_chunk(path, lx, lz)? else {
        println!("chunk absent");
        return Ok(());
    };
    let (_, t) = nbt::read_root(&raw)?;
    let Some(Tag::List(secs)) = t.get("sections") else { return Ok(()) };
    let idx_xz = (z.rem_euclid(16) * 16 + x.rem_euclid(16)) as usize;
    for y in (y0..=y1).rev() {
        let sy = y.div_euclid(16);
        let name = secs
            .iter()
            .find(|s| matches!(s.get("Y"), Some(Tag::Byte(b)) if *b as i32 == sy))
            .map(|s| block_names(s)[(y.rem_euclid(16) as usize) * 256 + idx_xz].clone())
            .unwrap_or_else(|| "(no section)".into());
        println!("  Y={y}: {name}");
    }
    for k in ["MOTION_BLOCKING", "OCEAN_FLOOR", "WORLD_SURFACE"] {
        if let Some(Tag::LongArray(a)) = t.get("Heightmaps").and_then(|h| h.get(k)) {
            let v = (((a[idx_xz / 5] as u64) >> ((idx_xz % 5) * 12)) & 0xfff) as i64;
            println!("  {k}: top Y = {}", v - 1 - 2032);
        }
    }
    Ok(())
}

/// Colour of a land-cover category (for feature renders).
fn cat_color(cat: &str) -> [u8; 3] {
    match cat {
        "plantation" => [30, 90, 45],
        "forest" => [60, 130, 60],
        "bamboo" => [120, 170, 60],
        "shrub" => [130, 160, 90],
        "grass" => [170, 200, 110],
        "alpine" => [170, 160, 140],
        "farm" => [200, 180, 100],
        "paddy" => [110, 190, 200],
        "tea" => [40, 160, 40],
        "orchard" => [210, 140, 80],
        "greenhouse" => [230, 230, 250],
        "solar" => [40, 40, 90],
        "urban" => [200, 190, 190],
        "residential" => [220, 210, 170],
        "industrial" => [170, 140, 170],
        "park" => [140, 210, 120],
        "cemetery" => [130, 130, 130],
        "golf" => [150, 230, 120],
        "airport" => [90, 90, 90],
        "port" => [150, 150, 170],
        "bare" | "quarry" => [190, 170, 140],
        "sand" => [230, 220, 170],
        "rock" => [120, 120, 120],
        "wetland" => [120, 160, 130],
        "water" => [50, 90, 200],
        "parking" => [100, 100, 100],
        "sports" => [200, 120, 100],
        _ => [0, 0, 0],
    }
}

/// Render a window of the feature layers: land-cover categories with
/// buildings (dark red) on top. `x0 z0 w h` in MC coordinates, 1 px = `s` m.
pub fn render_features(feat_dir: &Path, classes_csv: &Path, x0: i64, z0: i64, w: i64, h: i64, s: i64, out: &Path) -> Result<()> {
    let csv = std::fs::read_to_string(classes_csv)?;
    let mut cats = vec![String::new()];
    let lcmap = std::fs::read_to_string(crate::DEFAULT_LCMAP)?;
    let mut cat_of: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for l in lcmap.lines().skip(1) {
        let c: Vec<&str> = l.split(',').collect();
        if c.len() > 2 {
            cat_of.insert(c[0].to_string(), c[2].to_string());
        }
    }
    for l in csv.lines().skip(1) {
        let c: Vec<&str> = l.split(',').collect();
        if c.len() > 1 {
            cats.push(cat_of.get(c[1]).cloned().unwrap_or_default());
        }
    }
    let (pw, ph) = ((w / s) as usize, (h / s) as usize);
    let mut rgb = vec![0u8; pw * ph * 3];
    let tile = crate::heightmap::TILE;
    type Layers = (Vec<u16>, Vec<u32>, Vec<u8>, Vec<u8>);
    let mut cache: std::collections::HashMap<(i64, i64), Option<Layers>> = std::collections::HashMap::new();
    for py in 0..ph {
        for px in 0..pw {
            let (x, z) = (x0 + px as i64 * s, z0 + py as i64 * s);
            let (tx, tz) = (x.div_euclid(tile), z.div_euclid(tile));
            let e = cache.entry((tx, tz)).or_insert_with(|| {
                let open = |l: &str| crate::tiff::Reader::open(&feat_dir.join(crate::features::layer_name(l, tx * tile, tz * tile, "tif"))).ok();
                let (lr, br) = (open("lc")?, open("bldg")?);
                let (rr, ar) = (open("road"), open("rail"));
                let nn = (tile * tile) as usize;
                let (mut lc, mut bl, mut rd, mut ra) = (vec![0u16; nn], vec![0u32; nn], vec![0u8; nn], vec![0u8; nn]);
                for by in 0..20 {
                    for bx in 0..20 {
                        let a = lr.tile(bx, by).ok()?;
                        let b = br.tile(bx, by).ok()?;
                        let r8 = rr.as_ref().and_then(|r| r.tile(bx, by).ok());
                        let a8 = ar.as_ref().and_then(|r| r.tile(bx, by).ok());
                        for r in 0..512 {
                            for c in 0..512 {
                                let i = (by * 512 + r) * tile as usize + bx * 512 + c;
                                let k = r * 512 + c;
                                lc[i] = u16::from_le_bytes([a[2 * k], a[2 * k + 1]]);
                                bl[i] = u32::from_le_bytes(b[4 * k..4 * k + 4].try_into().unwrap());
                                if let Some(v) = &r8 {
                                    rd[i] = v[k];
                                }
                                if let Some(v) = &a8 {
                                    ra[i] = v[k];
                                }
                            }
                        }
                    }
                }
                Some((lc, bl, rd, ra))
            });
            let Some((lc, bl, rd, ra)) = e else { continue };
            let i = (z.rem_euclid(tile) * tile + x.rem_euclid(tile)) as usize;
            let col = if bl[i] != 0 {
                [150, 30, 30]
            } else if ra[i] != 0 {
                match ra[i] {
                    1 => [20, 20, 20],
                    2 => [120, 110, 100],
                    3 => [200, 200, 210],
                    _ => [230, 140, 40],
                }
            } else if rd[i] != 0 {
                match rd[i] {
                    1 => [40, 40, 60],
                    2 => [80, 80, 90],
                    3 => [125, 125, 125],
                    4 => [160, 160, 160],
                    5 => [190, 190, 185],
                    6 => [215, 200, 180],
                    7 => [225, 225, 215],
                    8 => [160, 120, 70],
                    9 => [150, 140, 120],
                    10 => [200, 180, 160],
                    11 => [255, 255, 255],
                    12 => [250, 210, 0],
                    _ => [255, 0, 255],
                }
            } else {
                cat_color(cats.get(lc[i] as usize).map(|s| s.as_str()).unwrap_or(""))
            };
            let k = (py * pw + px) * 3;
            rgb[k..k + 3].copy_from_slice(&col);
        }
    }
    crate::raster::write_png(out, pw, ph, &rgb)
}

pub fn cmd(args: &[String]) -> Result<()> {
    match args.first().map(|s| s.as_str()) {
        Some("striplight") => {
            let n = strip_light(Path::new(&args[1]), Path::new(&args[2]), args.get(3).is_some_and(|s| s == "heightmaps")).context("strip")?;
            println!("stripped light from {n} chunks");
        }
        Some("render") => {
            let v: Vec<i64> = args[2..6].iter().map(|s| s.parse().unwrap()).collect();
            render(Path::new(&args[1]), v[0], v[1], v[2], v[3], Path::new(&args[6]), args.get(7).and_then(|s| s.parse().ok()).unwrap_or(1))?
        }
        Some("featrender") => {
            let v: Vec<i64> = args[3..8].iter().map(|s| s.parse().unwrap()).collect();
            render_features(Path::new(&args[1]), Path::new(&args[2]), v[0], v[1], v[2], v[3], v[4], Path::new(&args[8]))?
        }
        Some("column") => {
            let v: Vec<i64> = args[2..6].iter().map(|s| s.parse().unwrap()).collect();
            column(Path::new(&args[1]), v[0], v[1], v[2] as i32, v[3] as i32)?
        }
        Some("view") => {
            // view <region dir> X Y Z yaw pitch out.png [W H fov maxdist]
            let f: Vec<f64> = args[2..7].iter().map(|s| s.parse().unwrap()).collect();
            let opt = |i: usize, d: f64| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
            let cam = crate::view::Cam { pos: [f[0], f[1], f[2]], yaw: f[3], pitch: f[4], w: opt(8, 1600.0) as usize, h: opt(9, 900.0) as usize, fov: opt(10, 70.0), maxd: opt(11, 400.0) };
            crate::view::render(Path::new(&args[1]), &cam, Path::new(&args[7]))?
        }
        Some("blockcmp") => compare_blocks(Path::new(&args[1]), Path::new(&args[2]))?,
        Some("lightcmp") => compare_light(Path::new(&args[1]), Path::new(&args[2]), args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10))?,
        _ => anyhow::bail!("verify striplight <src.mca> <dst.mca> | lightcmp <ours.mca> <vanilla.mca> [n]"),
    }
    Ok(())
}
