//! Measured trees for Shizuoka.
//!
//! * 静岡県森林資源解析データ (林相識別図以外): 単木ポイント — one point per tree
//!   detected in the prefecture's airborne LiDAR, with height and species —
//!   and 解析範囲ポリゴン, the analysed extent.
//! * VIRTUAL SHIZUOKA LP Original point clouds (LAS) where they were
//!   downloaded: a canopy height model (CHM) at 1 m, from non-ground points of
//!   multi-return pulses (vegetation) minus the LAS ground surface.
//!
//! Outputs under `<work>/trees/` (per heightmap tile):
//!   fpts_X.._Z...bin   forest points: [lx u16, lz u16, height dm u16, species u8, 0]
//!   chm_X.._Z...tif    U8 canopy height in 0.2 m units (0 = none / not vegetation)
//!   chmcov_X.._Z...tif U8 1 where the LAS covers the pixel
//! (The analysed extent polygons are only counted: coverage of measured
//! planted forest is taken from the points themselves, see features.rs.)

use crate::heightmap::TILE;
use crate::proj::{Lcc, Tm};
use crate::vfeat::VFeat;
use anyhow::{Result, bail};
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// species codes
pub const SP_UNKNOWN: u8 = 0;
pub const SP_CEDAR: u8 = 1;
pub const SP_CYPRESS: u8 = 2;
pub const SP_PINE: u8 = 3;
pub const SP_LARCH: u8 = 4;
pub const SP_FIR: u8 = 5;
pub const SP_BROADLEAF: u8 = 6;
pub const SP_BAMBOO: u8 = 7;
pub const SP_EVERGREEN: u8 = 8;

pub fn species_code(name: &str) -> u8 {
    let n = name.trim();
    if n.contains("カラマツ") {
        SP_LARCH
    } else if n.contains("スギ") {
        SP_CEDAR
    } else if n.contains("ヒノキ") || n.contains("サワラ") {
        SP_CYPRESS
    } else if n.contains("マツ") {
        SP_PINE
    } else if n.contains('竹') || n.contains("タケ") {
        SP_BAMBOO
    } else if n.contains("常緑") || n.contains("クス") || n.contains("シイ") || n.contains("カシ") {
        SP_EVERGREEN
    } else if n.contains("針葉") || n.ends_with('N') || n.ends_with('Ｎ') || n.contains("モミ") || n.contains("ツガ") {
        SP_FIR
    } else if n.contains("広葉") || n.ends_with('L') || n.ends_with('Ｌ') || n.contains("落葉") {
        SP_BROADLEAF
    } else {
        SP_UNKNOWN
    }
}

/// Species name used by the tree builder (rgen::tree_voxels).
pub fn species_name(code: u8, elev: f32, x: i64, z: i64) -> Option<&'static str> {
    Some(match code {
        SP_CEDAR => "cedar",
        SP_CYPRESS => "cypress",
        SP_PINE => "pine",
        SP_LARCH => "larch",
        SP_FIR => "fir",
        SP_BAMBOO => "bamboo",
        SP_EVERGREEN => "evergreen_broadleaf",
        SP_BROADLEAF => {
            // lowland broadleaf forest in Shizuoka is mostly evergreen (シイ・カシ)
            let p = crate::rgen::unit(x, z, 61);
            if elev < 500.0 && p < 0.6 || elev < 900.0 && p < 0.25 {
                "evergreen_broadleaf"
            } else if elev > 1300.0 && p < 0.5 {
                "beech"
            } else {
                "broadleaf"
            }
        }
        _ => return None,
    })
}

fn decode_name(raw: &[u8]) -> String {
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::SHIFT_JIS.decode(raw).0.into_owned(),
    }
}

/// Minimal DBF reader: field names and per-record string values.
struct Dbf {
    fields: Vec<(String, usize, usize)>, // name, offset in record, length
    nrec: usize,
    hlen: usize,
    rlen: usize,
    data: Vec<u8>,
}

impl Dbf {
    fn parse(data: Vec<u8>) -> Result<Dbf> {
        if data.len() < 32 {
            bail!("dbf too short");
        }
        let nrec = u32::from_le_bytes(data[4..8].try_into()?) as usize;
        let hlen = u16::from_le_bytes(data[8..10].try_into()?) as usize;
        let rlen = u16::from_le_bytes(data[10..12].try_into()?) as usize;
        let mut fields = Vec::new();
        let mut off = 32;
        let mut pos = 1;
        while off + 32 <= data.len() && data[off] != 0x0D {
            let raw = &data[off..off + 11];
            let end = raw.iter().position(|&c| c == 0).unwrap_or(11);
            let name = encoding_rs::SHIFT_JIS.decode(&raw[..end]).0.into_owned();
            let len = data[off + 16] as usize;
            fields.push((name, pos, len));
            pos += len;
            off += 32;
        }
        Ok(Dbf { fields, nrec, hlen, rlen, data })
    }
    fn field(&self, pred: impl Fn(&str) -> bool) -> Option<usize> {
        self.fields.iter().position(|f| pred(&f.0))
    }
    fn get(&self, r: usize, f: usize) -> String {
        let (_, o, l) = self.fields[f];
        let s = self.hlen + r * self.rlen + o;
        if s + l > self.data.len() {
            return String::new();
        }
        encoding_rs::SHIFT_JIS.decode(&self.data[s..s + l]).0.trim().to_string()
    }
}

/// Points of a Point/PointZ/PointM shapefile.
fn shp_points(d: &[u8]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut p = 100;
    while p + 12 <= d.len() {
        let clen = i32::from_be_bytes(d[p + 4..p + 8].try_into().unwrap()) as usize * 2;
        let st = i32::from_le_bytes(d[p + 8..p + 12].try_into().unwrap());
        if matches!(st, 1 | 11 | 21) && p + 28 <= d.len() {
            let x = f64::from_le_bytes(d[p + 12..p + 20].try_into().unwrap());
            let y = f64::from_le_bytes(d[p + 20..p + 28].try_into().unwrap());
            out.push((x, y));
        } else {
            out.push((f64::NAN, f64::NAN));
        }
        p += 8 + clen;
    }
    out
}

/// Rings of a Polygon/PolygonZ/PolygonM shapefile (one Vec of rings per record).
fn shp_polygons(d: &[u8]) -> Vec<Vec<Vec<(f64, f64)>>> {
    let mut out = Vec::new();
    let mut p = 100;
    while p + 12 <= d.len() {
        let clen = i32::from_be_bytes(d[p + 4..p + 8].try_into().unwrap()) as usize * 2;
        let st = i32::from_le_bytes(d[p + 8..p + 12].try_into().unwrap());
        let c = p + 8;
        let mut rings = Vec::new();
        if matches!(st, 5 | 15 | 25) && c + 44 <= d.len() {
            let np = i32::from_le_bytes(d[c + 36..c + 40].try_into().unwrap()) as usize;
            let npt = i32::from_le_bytes(d[c + 40..c + 44].try_into().unwrap()) as usize;
            let parts: Vec<usize> = (0..np).map(|k| i32::from_le_bytes(d[c + 44 + 4 * k..c + 48 + 4 * k].try_into().unwrap()) as usize).collect();
            let pts0 = c + 44 + 4 * np;
            for k in 0..np {
                let (a, b) = (parts[k], if k + 1 < np { parts[k + 1] } else { npt });
                let ring: Vec<(f64, f64)> = (a..b)
                    .map(|j| {
                        let q = pts0 + 16 * j;
                        (f64::from_le_bytes(d[q..q + 8].try_into().unwrap()), f64::from_le_bytes(d[q + 8..q + 16].try_into().unwrap()))
                    })
                    .collect();
                rings.push(ring);
            }
        }
        out.push(rings);
        p += 8 + clen;
    }
    out
}

fn tile_name(prefix: &str, tx: i64, tz: i64, ext: &str) -> String {
    format!("{prefix}_X{:+07}_Z{:+07}.{ext}", tx * TILE, tz * TILE)
}

/// One forest point: MC pixel, height (dm), species.
type FPt = (i64, i64, u16, u8);

fn forest_zip(path: &Path, tm: &Tm, lcc: &Lcc) -> Result<(Vec<FPt>, Vec<VFeat>, HashMap<String, usize>)> {
    let f = std::fs::File::open(path)?;
    let mut za = zip::ZipArchive::new(std::io::BufReader::new(f))?;
    let mut names: Vec<(usize, String)> = Vec::new();
    for i in 0..za.len() {
        let e = za.by_index_raw(i)?;
        names.push((i, decode_name(e.name_raw())));
    }
    let find = |stem: &str, ext: &str| names.iter().find(|(_, n)| n.len() > 4 && n[..n.len() - 4] == *stem && n.to_ascii_lowercase().ends_with(ext)).map(|e| e.0);
    let mut pts = Vec::new();
    let mut polys = Vec::new();
    let mut species: HashMap<String, usize> = HashMap::new();
    for (i, n) in names.clone() {
        if !n.to_ascii_lowercase().ends_with(".shp") {
            continue;
        }
        let stem = &n[..n.len() - 4];
        let is_tree = n.contains("単木ポイント");
        let is_extent = n.contains("解析範囲");
        if !is_tree && !is_extent {
            continue;
        }
        let mut shp = Vec::new();
        za.by_index(i)?.read_to_end(&mut shp)?;
        let to_mc = |x: f64, y: f64| {
            let (la, lo) = tm.inverse(x, y);
            lcc.ll_to_mc(la, lo)
        };
        if is_extent {
            for rings in shp_polygons(&shp) {
                let parts: Vec<Vec<[f64; 2]>> = rings
                    .iter()
                    .map(|r| {
                        r.iter()
                            .map(|&(x, y)| {
                                let (mx, mz) = to_mc(x, y);
                                [mx, mz]
                            })
                            .collect()
                    })
                    .filter(|r: &Vec<[f64; 2]>| r.len() >= 3)
                    .collect();
                if !parts.is_empty() {
                    polys.push(VFeat { area: true, parts, ..Default::default() });
                }
            }
            continue;
        }
        let Some(di) = find(stem, ".dbf") else { continue };
        let mut dbf = Vec::new();
        za.by_index(di)?.read_to_end(&mut dbf)?;
        let dbf = Dbf::parse(dbf)?;
        let fh = dbf.field(|s| s == "樹高");
        let fs = dbf.field(|s| s == "中樹種").or_else(|| dbf.field(|s| s.contains("樹種") && !s.contains("ID")));
        let Some(fh) = fh else {
            eprintln!("trees: {}: no 樹高 field in {n}", path.display());
            continue;
        };
        let xy = shp_points(&shp);
        for (r, &(x, y)) in xy.iter().enumerate().take(dbf.nrec) {
            if !x.is_finite() {
                continue;
            }
            let h: f32 = dbf.get(r, fh).parse().unwrap_or(0.0);
            if !(1.0..80.0).contains(&h) {
                continue;
            }
            let sname = fs.map(|f| dbf.get(r, f)).unwrap_or_default();
            let sp = species_code(&sname);
            *species.entry(sname).or_default() += 1;
            let (mx, mz) = to_mc(x, y);
            pts.push((mx.floor() as i64, mz.floor() as i64, (h * 10.0).round() as u16, sp));
        }
    }
    Ok((pts, polys, species))
}

/// Forest analysis points and extent.
pub fn ingest_forest(dir: &Path, work: &Path) -> Result<()> {
    let t0 = std::time::Instant::now();
    let mut zips: Vec<PathBuf> = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")) {
                zips.push(p);
            }
        }
    }
    zips.sort();
    let tm = Tm::zone8();
    let lcc = Lcc::new();
    let res: Vec<(Vec<FPt>, Vec<VFeat>, HashMap<String, usize>)> = zips.par_iter().map(|z| forest_zip(z, &tm, &lcc)).collect::<Result<_>>()?;
    let mut by_tile: HashMap<(i64, i64), Vec<FPt>> = HashMap::new();
    let mut polys = Vec::new();
    let mut species: HashMap<String, usize> = HashMap::new();
    let mut total = 0usize;
    for (pts, pl, sp) in res {
        total += pts.len();
        for p in pts {
            by_tile.entry((p.0.div_euclid(TILE), p.1.div_euclid(TILE))).or_default().push(p);
        }
        polys.extend(pl);
        for (k, v) in sp {
            *species.entry(k).or_default() += v;
        }
    }
    let out = work.join("trees");
    std::fs::create_dir_all(&out)?;
    let mut kept = 0usize;
    for ((tx, tz), mut v) in by_tile {
        // one tree per pixel (the tallest) where analyses overlap
        v.sort_by_key(|p| (p.1, p.0, std::cmp::Reverse(p.2)));
        v.dedup_by_key(|p| (p.0, p.1));
        kept += v.len();
        let mut b = Vec::with_capacity(v.len() * 8);
        for (x, z, h, s) in v {
            b.extend_from_slice(&((x - tx * TILE) as u16).to_le_bytes());
            b.extend_from_slice(&((z - tz * TILE) as u16).to_le_bytes());
            b.extend_from_slice(&h.to_le_bytes());
            b.push(s);
            b.push(0);
        }
        write_if_changed(&out.join(tile_name("fpts", tx, tz, "bin")), &b)?;
    }
    let mut sp: Vec<(String, usize)> = species.into_iter().collect();
    sp.sort_by(|a, b| b.1.cmp(&a.1));
    eprintln!("trees: {} zips, {total} tree points ({kept} kept), {} extent polygons in {:.1}s", zips.len(), polys.len(), t0.elapsed().as_secs_f64());
    for (k, v) in sp.iter().take(25) {
        eprintln!("  {v:>10} {k} -> {}", species_code(k));
    }
    Ok(())
}

fn write_if_changed(p: &Path, b: &[u8]) -> Result<bool> {
    if std::fs::read(p).map(|d| d == b).unwrap_or(false) {
        return Ok(false);
    }
    std::fs::write(p, b)?;
    Ok(true)
}

pub fn read_fpts(work: &Path, tx: i64, tz: i64) -> Vec<(i64, i64, f32, u8)> {
    let Ok(d) = std::fs::read(work.join("trees").join(tile_name("fpts", tx, tz, "bin"))) else { return vec![] };
    d.chunks_exact(8)
        .map(|c| {
            let lx = u16::from_le_bytes([c[0], c[1]]) as i64;
            let lz = u16::from_le_bytes([c[2], c[3]]) as i64;
            (tx * TILE + lx, tz * TILE + lz, u16::from_le_bytes([c[4], c[5]]) as f32 / 10.0, c[6])
        })
        .collect()
}

// ---------------------------------------------------------------- LAS → CHM

struct LasTile {
    /// MC pixel origin and size of the local grid
    x0: i64,
    z0: i64,
    w: usize,
    h: usize,
    chm: Vec<u8>,
    cov: Vec<u8>,
}

fn las_tile(path: &Path, tm: &Tm, lcc: &Lcc) -> Result<Option<LasTile>> {
    let f = std::fs::File::open(path)?;
    let mut za = zip::ZipArchive::new(std::io::BufReader::new(f))?;
    let idx = (0..za.len()).find(|&i| za.by_index_raw(i).map(|e| e.name().to_ascii_lowercase().ends_with(".las")).unwrap_or(false));
    let Some(idx) = idx else { return Ok(None) };
    let mut r = std::io::BufReader::with_capacity(1 << 22, za.by_index(idx)?);
    let mut hdr = vec![0u8; 227];
    r.read_exact(&mut hdr)?;
    if &hdr[0..4] != b"LASF" {
        bail!("{}: not LAS", path.display());
    }
    let off = u32::from_le_bytes(hdr[96..100].try_into()?) as usize;
    let fmt = hdr[104] & 0x3f;
    let rlen = u16::from_le_bytes(hdr[105..107].try_into()?) as usize;
    let mut n = u32::from_le_bytes(hdr[107..111].try_into()?) as u64;
    let g = |o: usize| f64::from_le_bytes(hdr[o..o + 8].try_into().unwrap());
    let (sx, sy, sz, ox, oy, oz) = (g(131), g(139), g(147), g(155), g(163), g(171));
    let (maxx, minx, maxy, miny) = (g(179), g(187), g(195), g(203));
    let mut rest = vec![0u8; off - 227];
    r.read_exact(&mut rest)?;
    if hdr[24] == 1 && hdr[25] >= 4 && rest.len() >= 255 - 227 + 8 {
        let n14 = u64::from_le_bytes(rest[247 - 227..255 - 227].try_into()?);
        if n14 > 0 {
            n = n14;
        }
    }
    // zone VIII → MC affine from three corners
    let to_mc = |x: f64, y: f64| {
        let (la, lo) = tm.inverse(x, y);
        lcc.ll_to_mc(la, lo)
    };
    let p0 = to_mc(minx, miny);
    let px = to_mc(maxx, miny);
    let py = to_mc(minx, maxy);
    let (dx, dy) = ((maxx - minx).max(1e-6), (maxy - miny).max(1e-6));
    let (ax, az) = ((px.0 - p0.0) / dx, (px.1 - p0.1) / dx);
    let (bx, bz) = ((py.0 - p0.0) / dy, (py.1 - p0.1) / dy);
    let corners = [p0, px, py, to_mc(maxx, maxy)];
    let x0 = corners.iter().map(|c| c.0).fold(f64::MAX, f64::min).floor() as i64 - 1;
    let z0 = corners.iter().map(|c| c.1).fold(f64::MAX, f64::min).floor() as i64 - 1;
    let x1 = corners.iter().map(|c| c.0).fold(f64::MIN, f64::max).ceil() as i64 + 1;
    let z1 = corners.iter().map(|c| c.1).fold(f64::MIN, f64::max).ceil() as i64 + 1;
    let (w, h) = ((x1 - x0) as usize, (z1 - z0) as usize);
    let mut top = vec![f32::NEG_INFINITY; w * h];
    let mut n_ng = vec![0u16; w * h];
    let mut n_multi = vec![0u16; w * h];
    let mut gsum = vec![0f32; w * h];
    let mut gcnt = vec![0u16; w * h];
    let (cls_at, ret_new) = if fmt >= 6 { (16usize, true) } else { (15usize, false) };
    let mut buf = vec![0u8; rlen * 65536];
    let mut left = n as usize;
    while left > 0 {
        let k = left.min(65536);
        r.read_exact(&mut buf[..k * rlen])?;
        for p in buf[..k * rlen].chunks_exact(rlen) {
            let xi = i32::from_le_bytes(p[0..4].try_into().unwrap());
            let yi = i32::from_le_bytes(p[4..8].try_into().unwrap());
            let zi = i32::from_le_bytes(p[8..12].try_into().unwrap());
            let (x, y, z) = (xi as f64 * sx + ox - minx, yi as f64 * sy + oy - miny, (zi as f64 * sz + oz) as f32);
            let mx = p0.0 + ax * x + bx * y;
            let mz = p0.1 + az * x + bz * y;
            let (ix, iz) = (mx.floor() as i64 - x0, mz.floor() as i64 - z0);
            if ix < 0 || iz < 0 || ix >= w as i64 || iz >= h as i64 {
                continue;
            }
            let i = iz as usize * w + ix as usize;
            let class = p[cls_at] & if ret_new { 0xff } else { 0x1f };
            let nret = if ret_new { p[14] >> 4 } else { (p[14] >> 3) & 7 };
            match class {
                2 => {
                    gsum[i] += z;
                    gcnt[i] = gcnt[i].saturating_add(1);
                }
                7 | 18 => {} // noise
                _ => {
                    top[i] = top[i].max(z);
                    n_ng[i] = n_ng[i].saturating_add(1);
                    if nret >= 2 {
                        n_multi[i] = n_multi[i].saturating_add(1);
                    }
                }
            }
        }
        left -= k;
    }
    // ground surface: per-pixel mean, gaps filled from neighbours
    let mut gnd: Vec<f32> = (0..w * h).map(|i| if gcnt[i] > 0 { gsum[i] / gcnt[i] as f32 } else { f32::NAN }).collect();
    for _ in 0..40 {
        let prev = gnd.clone();
        let mut missing = 0;
        for zz in 0..h {
            for xx in 0..w {
                let i = zz * w + xx;
                if prev[i].is_finite() {
                    continue;
                }
                let (mut s, mut c) = (0f32, 0);
                for (ddx, ddz) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1), (-1, -1), (1, 1), (-1, 1), (1, -1)] {
                    let (a, b) = (xx as i64 + ddx, zz as i64 + ddz);
                    if a >= 0 && b >= 0 && (a as usize) < w && (b as usize) < h {
                        let v = prev[b as usize * w + a as usize];
                        if v.is_finite() {
                            s += v;
                            c += 1;
                        }
                    }
                }
                if c > 0 {
                    gnd[i] = s / c as f32;
                } else {
                    missing += 1;
                }
            }
        }
        if missing == 0 {
            break;
        }
    }
    let mut chm = vec![0u8; w * h];
    let mut cov = vec![0u8; w * h];
    for i in 0..w * h {
        if n_ng[i] > 0 || gcnt[i] > 0 {
            cov[i] = 1;
        }
        // trees: dense non-ground echoes (wires and poles give only a few), a
        // fair share of them from multi-return pulses (roofs give none);
        // below 2 m any dense low cover (tea hedges, shrubs, crops)
        if n_ng[i] >= 4 && gnd[i].is_finite() {
            let ht = top[i] - gnd[i];
            let veg = n_ng[i] >= 6 && n_multi[i] as f32 >= 0.12 * n_ng[i] as f32;
            if (ht >= 2.0 && veg) || (0.3..2.0).contains(&ht) {
                chm[i] = (ht * 5.0).round().clamp(1.0, 255.0) as u8;
            }
        }
    }
    Ok(Some(LasTile { x0, z0, w, h, chm, cov }))
}

/// Canopy height model from all LAS zips under `dir`.
pub fn ingest_las(dir: &Path, work: &Path) -> Result<()> {
    let t0 = std::time::Instant::now();
    let mut zips: Vec<PathBuf> = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")) {
                zips.push(p);
            }
        }
    }
    zips.sort();
    let tm = Tm::zone8();
    let lcc = Lcc::new();
    let tiles: Mutex<HashMap<(i64, i64), (Vec<u8>, Vec<u8>)>> = Mutex::new(HashMap::new());
    let done = std::sync::atomic::AtomicUsize::new(0);
    zips.par_iter().try_for_each(|z| -> Result<()> {
        let Some(t) = las_tile(z, &tm, &lcc)? else { return Ok(()) };
        let mut m = tiles.lock().unwrap();
        let tsz = TILE as usize;
        for zz in 0..t.h {
            for xx in 0..t.w {
                let i = zz * t.w + xx;
                if t.cov[i] == 0 {
                    continue;
                }
                let (x, zg) = (t.x0 + xx as i64, t.z0 + zz as i64);
                let (tx, tz) = (x.div_euclid(TILE), zg.div_euclid(TILE));
                let e = m.entry((tx, tz)).or_insert_with(|| (vec![0u8; tsz * tsz], vec![0u8; tsz * tsz]));
                let j = (zg - tz * TILE) as usize * tsz + (x - tx * TILE) as usize;
                e.0[j] = e.0[j].max(t.chm[i]);
                e.1[j] = 1;
            }
        }
        let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if d % 20 == 0 {
            eprintln!("  las: {d}/{} tiles ({:.0}s)", zips.len(), t0.elapsed().as_secs_f64());
        }
        Ok(())
    })?;
    let out = work.join("trees");
    std::fs::create_dir_all(&out)?;
    let tsz = TILE as usize;
    for ((tx, tz), (chm, cov)) in tiles.into_inner().unwrap() {
        crate::tiff::write(&out.join(tile_name("chm", tx, tz, "tif")), crate::tiff::Fmt::U8, tsz, tsz, &chm, tx * TILE, tz * TILE, Some("0"), 6)?;
        crate::tiff::write(&out.join(tile_name("chmcov", tx, tz, "tif")), crate::tiff::Fmt::U8, tsz, tsz, &cov, tx * TILE, tz * TILE, Some("0"), 6)?;
        let veg = chm.iter().filter(|&&v| v > 0).count();
        let c = cov.iter().filter(|&&v| v > 0).count();
        eprintln!("las: tile X={} Z={}: {c} px covered, {veg} vegetation px", tx * TILE, tz * TILE);
    }
    eprintln!("las: {} LAS tiles in {:.1}s", zips.len(), t0.elapsed().as_secs_f64());
    Ok(())
}

/// CHM layers of a heightmap tile (None where no LAS was processed).
pub fn read_chm(work: &Path, tx: i64, tz: i64) -> Option<(Vec<u8>, Vec<u8>)> {
    let rd = |name: &str| -> Option<Vec<u8>> {
        let r = crate::tiff::Reader::open(&work.join("trees").join(tile_name(name, tx, tz, "tif"))).ok()?;
        let t = TILE as usize;
        let mut v = vec![0u8; t * t];
        for by in 0..t / 512 {
            for bx in 0..t / 512 {
                let b = r.tile(bx, by).ok()?;
                for row in 0..512 {
                    let d = (by * 512 + row) * t + bx * 512;
                    v[d..d + 512].copy_from_slice(&b[row * 512..row * 512 + 512]);
                }
            }
        }
        Some(v)
    };
    Some((rd("chm")?, rd("chmcov")?))
}
