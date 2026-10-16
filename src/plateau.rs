//! PLATEAU CityGML (bldg) → building footprints with heights, usage and,
//! where LOD2 exists, 3D roof polygons. Reads the per-file downloads under
//! `<dir>/bldg/<city code>/*.gml[.gz]` (latest year per city, see README) and
//! city zips (only `udx/bldg/*.gml` entries) for cities without those files.
//!
//! Output store `vec_plateau_bldg.bin`: one VFeat per Building/BuildingPart:
//!   parts[..class] = footprint rings (MC X,Z; outer and inner rings of every
//!   polygon, filled even-odd so courtyards stay open); parts[class..] = LOD2
//!   roof polygons; per-vertex heights in `zs` (footprints: ground z)
//!   nums = [measuredHeight m, storeysAboveGround, usage code, ground z (T.P. m)]
//!   flags: 1 = has LOD2 roof

use crate::ingest::TILE_MARGIN;
use crate::proj::Lcc;
use crate::vfeat::{self, VFeat};
use anyhow::Result;
use rayon::prelude::*;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const F_LOD2: u16 = 1;

fn find(h: &[u8], pat: &[u8], from: usize) -> Option<usize> {
    if from >= h.len() || pat.is_empty() {
        return None;
    }
    let first = pat[0];
    let mut i = from;
    while i + pat.len() <= h.len() {
        match h[i..].iter().position(|&b| b == first) {
            None => return None,
            Some(p) => {
                i += p;
                if i + pat.len() <= h.len() && &h[i..i + pat.len()] == pat {
                    return Some(i);
                }
                i += 1;
            }
        }
    }
    None
}

fn text_of<'a>(h: &'a [u8], open: &str, from: usize) -> Option<&'a str> {
    let s = find(h, open.as_bytes(), from)?;
    let gt = find(h, b">", s)? + 1;
    let e = find(h, b"<", gt)?;
    std::str::from_utf8(&h[gt..e]).ok()
}

/// All gml:posList contents (3D: lat lon z triples) inside h[from..to].
fn poslists(h: &[u8], from: usize, to: usize) -> Vec<Vec<(f64, f64, f64)>> {
    let mut out = Vec::new();
    let mut p = from;
    while let Some(s) = find(&h[..to], b"<gml:posList", p) {
        let Some(gt) = find(h, b">", s) else { break };
        let Some(e) = find(h, b"</gml:posList>", gt) else { break };
        let txt = std::str::from_utf8(&h[gt + 1..e]).unwrap_or("");
        let v: Vec<f64> = txt.split_ascii_whitespace().filter_map(|x| x.parse().ok()).collect();
        let pts: Vec<(f64, f64, f64)> = v.chunks_exact(3).map(|c| (c[0], c[1], c[2])).collect();
        if pts.len() >= 3 {
            out.push(pts);
        }
        p = e;
    }
    out
}

/// Distinct rings (a ring published twice would cancel out in an even-odd fill).
fn rings_of(v: Vec<Vec<(f64, f64, f64)>>) -> Vec<Vec<(f64, f64, f64)>> {
    let mut out: Vec<Vec<(f64, f64, f64)>> = Vec::new();
    for r in v {
        let same = |o: &Vec<(f64, f64, f64)>| o.len() == r.len() && o.iter().zip(&r).all(|(a, b)| (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9);
        if !out.iter().any(same) {
            out.push(r);
        }
    }
    out
}

/// Byte ranges of every `<tag ...>...</tag>` inside h[from..to].
fn elements(h: &[u8], tag: &str, from: usize, to: usize) -> Vec<(usize, usize)> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut v = Vec::new();
    let mut p = from;
    while let Some(s) = find(&h[..to], open.as_bytes(), p) {
        // make sure it is exactly this tag (next char is space or '>')
        let c = h.get(s + open.len()).copied().unwrap_or(b' ');
        if c != b' ' && c != b'>' {
            p = s + 1;
            continue;
        }
        let Some(e) = find(&h[..to], close.as_bytes(), s) else { break };
        v.push((s, e + close.len()));
        p = e + close.len();
    }
    v
}

struct Bld {
    /// footprint rings (outer and inner, of every polygon; filled even-odd)
    footprint: Vec<Vec<(f64, f64, f64)>>,
    roofs: Vec<Vec<(f64, f64, f64)>>,
    height: f64,
    storeys: f64,
    usage: f64,
    ground: f64,
}

fn parse_building(h: &[u8], s: usize, e: usize, inherit_usage: f64) -> Option<Bld> {
    let num = |tag: &str| text_of(&h[..e], tag, s).and_then(|t| t.trim().parse::<f64>().ok());
    let usage = num("<bldg:usage").unwrap_or(inherit_usage);
    let height = num("<bldg:measuredHeight").unwrap_or(0.0);
    let storeys = num("<bldg:storeysAboveGround").filter(|v| *v < 9000.0).unwrap_or(0.0);
    // LOD2 roof polygons
    let mut roofs = Vec::new();
    for (rs, re) in elements(h, "bldg:RoofSurface", s, e) {
        roofs.extend(poslists(h, rs, re));
    }
    // footprint: lod0FootPrint / lod0RoofEdge / GroundSurface / lowest lod1 face
    let mut footprint: Option<Vec<Vec<(f64, f64, f64)>>> = None;
    for tag in ["bldg:lod0FootPrint", "bldg:lod0RoofEdge"] {
        if let Some(&(fs, fe)) = elements(h, tag, s, e).first() {
            let rings = rings_of(poslists(h, fs, fe));
            if !rings.is_empty() {
                footprint = Some(rings);
                break;
            }
        }
    }
    let mut ground = f64::NAN;
    let mut top = f64::NAN;
    for tag in ["bldg:lod1Solid", "bldg:lod2Solid"] {
        if let Some(&(ls, le)) = elements(h, tag, s, e).first() {
            let faces = poslists(h, ls, le);
            for f in &faces {
                for &(_, _, z) in f {
                    ground = if ground.is_nan() { z } else { ground.min(z) };
                    top = if top.is_nan() { z } else { top.max(z) };
                }
            }
            if footprint.is_none() {
                // the face whose vertices all lie at the minimum height
                footprint = faces.into_iter().find(|f| f.iter().all(|p| (p.2 - ground).abs() < 0.01)).map(|f| vec![f]);
            }
            break;
        }
    }
    if let Some(&(gs, ge)) = elements(h, "bldg:GroundSurface", s, e).first() {
        let g = rings_of(poslists(h, gs, ge));
        if !g.is_empty() {
            if ground.is_nan() {
                ground = g.iter().flatten().map(|p| p.2).fold(f64::INFINITY, f64::min);
            }
            if footprint.is_none() {
                footprint = Some(g);
            }
        }
    }
    let footprint = footprint?;
    let height = if height > 0.0 { height } else if !top.is_nan() { top - ground } else { 0.0 };
    Some(Bld { footprint, roofs, height, storeys, usage, ground: if ground.is_nan() { 0.0 } else { ground } })
}

/// Parse every building of one CityGML file. `id` of each feature is the
/// index of its bldg:Building in the file (parts share it).
fn parse_file(h: &[u8], lcc: &Lcc) -> Vec<VFeat> {
    let mut out = Vec::new();
    for (bi, (s, e)) in elements(h, "bldg:Building", 0, h.len()).into_iter().enumerate() {
        let parts = elements(h, "bldg:BuildingPart", s, e);
        let mut blds = Vec::new();
        if parts.is_empty() {
            blds.extend(parse_building(h, s, e, 0.0));
        } else {
            let usage = text_of(&h[..e], "<bldg:usage", s).and_then(|t| t.trim().parse().ok()).unwrap_or(0.0);
            for (ps, pe) in parts {
                blds.extend(parse_building(h, ps, pe, usage));
            }
        }
        for b in blds {
            let proj = |v: &Vec<(f64, f64, f64)>| -> (Vec<[f64; 2]>, Vec<f32>) {
                v.iter()
                    .map(|&(la, lo, z)| {
                        let (x, zz) = lcc.ll_to_mc(la, lo);
                        ([x, zz], z as f32)
                    })
                    .unzip()
            };
            let (mut parts, mut zs): (Vec<Vec<[f64; 2]>>, Vec<Vec<f32>>) = b.footprint.iter().map(proj).unzip();
            let nfoot = parts.len() as u16;
            for r in &b.roofs {
                let (p, z) = proj(r);
                parts.push(p);
                zs.push(z);
            }
            out.push(VFeat {
                class: nfoot,
                flags: if b.roofs.is_empty() { 0 } else { F_LOD2 },
                nums: [b.height as f32, b.storeys as f32, b.usage as f32, b.ground as f32],
                id: bi as u64,
                text: String::new(),
                area: true,
                parts,
                zs,
            });
        }
    }
    out
}

/// All `*citygml*.zip` under `dir` (one per city & year; newest year per city code wins).
pub fn city_zips(dir: &Path) -> Vec<PathBuf> {
    let mut all = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| {
                let n = n.to_string_lossy().to_ascii_lowercase();
                n.contains("citygml") && n.ends_with(".zip")
            }) {
                all.push(p);
            }
        }
    }
    // newest year per city code (file names start with the 5-digit code, contain _20xx_)
    let mut best: std::collections::HashMap<String, (String, PathBuf)> = std::collections::HashMap::new();
    for p in all {
        let n = p.file_name().unwrap().to_string_lossy().to_string();
        let code: String = n.chars().take(5).collect();
        let year = n.split('_').find(|s| s.len() == 4 && s.starts_with("20") && s.chars().all(|c| c.is_ascii_digit())).unwrap_or("0000").to_string();
        let e = best.entry(code).or_insert((String::new(), PathBuf::new()));
        if year > e.0 {
            *e = (year, p);
        }
    }
    let mut v: Vec<PathBuf> = best.into_values().map(|(_, p)| p).collect();
    v.sort();
    v
}

/// Per-file downloads: `<dir>/bldg/<5-digit city code>/<mesh>_bldg_*.gml[.gz]`.
pub fn city_files(dir: &Path) -> Vec<(u64, Vec<PathBuf>)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir.join("bldg")) else { return out };
    for e in rd.flatten() {
        let Ok(code) = e.file_name().to_string_lossy().parse::<u64>() else { continue };
        let mut files: Vec<PathBuf> = std::fs::read_dir(e.path())
            .into_iter()
            .flatten()
            .flatten()
            .map(|f| f.path())
            .filter(|p| {
                let n = p.to_string_lossy();
                n.ends_with(".gml") || n.ends_with(".gml.gz")
            })
            .collect();
        files.sort();
        if !files.is_empty() {
            out.push((code, files));
        }
    }
    out.sort();
    out
}

fn read_gml(p: &Path) -> Result<Vec<u8>> {
    let d = std::fs::read(p)?;
    if d.len() > 2 && d[0] == 0x1f && d[1] == 0x8b {
        let mut out = Vec::with_capacity(d.len() * 12);
        flate2::read::MultiGzDecoder::new(&d[..]).read_to_end(&mut out)?;
        Ok(out)
    } else {
        Ok(d)
    }
}

/// gml:id of every bldg:Building in a file, used to drop buildings that two
/// neighbouring municipalities both publish.
fn building_ids(h: &[u8]) -> Vec<u64> {
    elements(h, "bldg:Building", 0, h.len())
        .iter()
        .map(|&(s, _)| {
            let gt = find(h, b">", s).unwrap_or(s);
            let head = &h[s..gt];
            let id = find(head, b"gml:id=\"", 0).map(|i| {
                let v = &head[i + 8..];
                &v[..v.iter().position(|&c| c == b'"').unwrap_or(v.len())]
            });
            id.map(|v| v.iter().fold(0xcbf29ce484222325u64, |a, &c| (a ^ c as u64).wrapping_mul(0x100000001b3))).unwrap_or(0)
        })
        .collect()
}

pub fn run(dir: &Path, work: &Path) -> Result<()> {
    let t0 = std::time::Instant::now();
    let lcc = Lcc::new();
    let cities = city_files(dir);
    let have: std::collections::HashSet<u64> = cities.iter().map(|c| c.0).collect();
    // a city zip is superseded by per-file data of the same code or, for a
    // designated city / the 23 wards (code ..00/..30), of its wards (code+1..+29)
    let zips: Vec<PathBuf> = city_zips(dir)
        .into_iter()
        .filter(|z| {
            let code: u64 = z.file_name().unwrap().to_string_lossy().chars().take(5).collect::<String>().parse().unwrap_or(0);
            !(have.contains(&code) || (code % 10 == 0 && (code + 1..code + 30).any(|c| have.contains(&c))))
        })
        .collect();
    eprintln!("plateau: {} cities as per-file GML, {} city archives", cities.len(), zips.len());
    // Parse in parallel but merge in a fixed order (cities, then files), so the
    // output and therefore the feature rasters are identical from run to run.
    // A building published in two files (same gml:id) is kept from the first.
    let mut feats: Vec<VFeat> = Vec::new();
    let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut dup = 0usize;
    let mut merge = |city: u64, ids: Vec<u64>, local: Vec<VFeat>, feats: &mut Vec<VFeat>| -> usize {
        let mut keep: Vec<Option<bool>> = vec![None; ids.len()];
        let mut n = 0;
        for mut f in local {
            let bi = f.id as usize;
            let k = *keep[bi].get_or_insert_with(|| {
                let fresh = ids[bi] == 0 || seen.insert(ids[bi]);
                if !fresh {
                    dup += 1;
                }
                fresh
            });
            if k {
                f.id = city;
                feats.push(f);
                n += 1;
            }
        }
        n
    };
    for (city, files) in &cities {
        let t = std::time::Instant::now();
        let parsed: Vec<(Vec<u64>, Vec<VFeat>)> = files
            .par_iter()
            .map(|f| -> Result<(Vec<u64>, Vec<VFeat>)> {
                let buf = read_gml(f).map_err(|e| anyhow::anyhow!("{}: {e}", f.display()))?;
                Ok((building_ids(&buf), parse_file(&buf, &lcc)))
            })
            .collect::<Result<_>>()?;
        let mut n = 0;
        for (ids, local) in parsed {
            n += merge(*city, ids, local, &mut feats);
        }
        eprintln!("plateau: {city}: {} files, {n} buildings in {:.1}s", files.len(), t.elapsed().as_secs_f64());
    }
    for z in &zips {
        let t = std::time::Instant::now();
        let name = z.file_name().unwrap().to_string_lossy().to_string();
        let city: u64 = name.chars().take(5).collect::<String>().parse().unwrap_or(0);
        let f = std::fs::File::open(z)?;
        let za = zip::ZipArchive::new(std::io::BufReader::new(f))?;
        let mut entries: Vec<String> = za.file_names().filter(|n| n.contains("/bldg/") && n.ends_with(".gml")).map(|s| s.to_string()).collect();
        entries.sort();
        drop(za);
        let parsed: Vec<Vec<(Vec<u64>, Vec<VFeat>)>> = entries
            .par_chunks(64)
            .map(|chunk| -> Result<Vec<(Vec<u64>, Vec<VFeat>)>> {
                let f = std::fs::File::open(z)?;
                let mut za = zip::ZipArchive::new(std::io::BufReader::new(f))?;
                let mut v = Vec::new();
                for e in chunk {
                    let mut buf = Vec::new();
                    za.by_name(e)?.read_to_end(&mut buf)?;
                    v.push((building_ids(&buf), parse_file(&buf, &lcc)));
                }
                Ok(v)
            })
            .collect::<Result<_>>()?;
        let mut n = 0;
        for (ids, local) in parsed.into_iter().flatten() {
            n += merge(city, ids, local, &mut feats);
        }
        eprintln!("plateau: {name}: {} files, {n} buildings in {:.1}s", entries.len(), t.elapsed().as_secs_f64());
    }
    drop(merge);
    eprintln!("plateau: {dup} duplicate buildings (same gml:id in two files) skipped");
    let lod2 = feats.iter().filter(|f| f.flags & F_LOD2 != 0).count();
    vfeat::write_tiled(&work.join("vec_plateau_bldg.bin"), &feats, TILE_MARGIN)?;
    eprintln!("plateau: {} buildings ({} with LOD2 roofs) in {:.1}s", feats.len(), lod2, t0.elapsed().as_secs_f64());
    Ok(())
}
