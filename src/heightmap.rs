//! 1m heightmap tiles (10240 × 10240, aligned to multiples of 10240 in
//! Minecraft coordinates) as Float32 GeoTIFF, plus a uint8 class raster.

use crate::area::Area;
use crate::proj::{Lcc, LlGrid};
use crate::scan::WaterIndex;
use crate::store::{LevelStore, Lv, Sample, Window};
use crate::{gml::WaterKind, lakes, raster, tiff};
use anyhow::Result;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const TILE: i64 = 10240;
const HALO: i64 = 8;
pub const NODATA: f32 = -9999.0;
pub const SEA_FLOOR: f32 = -250.0;

// class raster codes
pub const K_OUT: u8 = 0;
pub const K_LAND1: u8 = 1; // land, 1m DEM
pub const K_LAND5: u8 = 2; // land, 5m DEM (1m missing)
pub const K_HOLE: u8 = 3; // land, interpolated (no DEM at all)
pub const K_SEA: u8 = 4;
pub const K_INLAND: u8 = 5; // lake / river surface
pub const CLASS_NAMES: [&str; 6] = ["outside", "land_1m", "land_5m", "land_filled", "sea", "inland_water"];

const M_SEA: u8 = 1;
const M_INLAND: u8 = 2;

pub fn height_name(x: i64, z: i64) -> String {
    format!("height_X{x:+07}_Z{z:+07}.tif")
}
pub fn class_name(x: i64, z: i64) -> String {
    format!("class_X{x:+07}_Z{z:+07}.tif")
}

struct MPoly {
    sea: bool,
    rings: Vec<Vec<(f64, f64)>>,
    x0: f64,
    x1: f64,
    z0: f64,
    z1: f64,
}

pub struct Ctx {
    pub area: Area,
    pub lcc: Lcc,
    m1: LevelStore,
    m5: LevelStore,
    wl: LevelStore,
    hf: LevelStore,
    polys: Vec<MPoly>,
    kihon: HashSet<(i64, i64)>,
}

impl Ctx {
    pub fn new(area: Area, work: &Path, wa: WaterIndex) -> Result<Self> {
        let lcc = Lcc::new();
        let polys: Vec<MPoly> = wa
            .polys
            .par_iter()
            .map(|p| {
                let rings: Vec<Vec<(f64, f64)>> = p.rings.iter().map(|r| r.iter().map(|&(la, lo)| lcc.ll_to_mc(la, lo)).collect()).collect();
                let (mut x0, mut x1, mut z0, mut z1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
                for &(x, z) in &rings[0] {
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                    z0 = z0.min(z);
                    z1 = z1.max(z);
                }
                MPoly { sea: p.kind == WaterKind::Sea, rings, x0, x1, z0, z1 }
            })
            .collect();
        Ok(Ctx {
            area,
            lcc,
            m1: LevelStore::open(Lv::M1, &work.join("cache_1m.bin"))?,
            m5: LevelStore::open(Lv::M5, &work.join("cache_5m.bin"))?,
            wl: LevelStore::open(Lv::M5, &work.join("cache_waterlevel.bin"))?,
            hf: LevelStore::open(Lv::M5, &work.join("cache_holefill.bin"))?,
            polys,
            kihon: lakes::mesh2_set(&wa.meshes),
        })
    }

    /// Tiles (tx, tz) that contain at least one pixel of the area.
    pub fn tile_list(&self) -> Vec<(i64, i64)> {
        let pts: Vec<(f64, f64)> = self.area.boundary_points(1e-5).par_iter().map(|&(la, lo)| self.lcc.ll_to_mc(la, lo)).collect();
        let (mut x0, mut x1, mut z0, mut z1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        let mut on_boundary: HashSet<(i64, i64)> = HashSet::new();
        for &(x, z) in &pts {
            x0 = x0.min(x);
            x1 = x1.max(x);
            z0 = z0.min(z);
            z1 = z1.max(z);
            on_boundary.insert(((x.floor() as i64).div_euclid(TILE), (z.floor() as i64).div_euclid(TILE)));
        }
        let mut v = Vec::new();
        for tz in (z0.floor() as i64).div_euclid(TILE)..=(z1.floor() as i64).div_euclid(TILE) {
            for tx in (x0.floor() as i64).div_euclid(TILE)..=(x1.floor() as i64).div_euclid(TILE) {
                let mut inside = on_boundary.contains(&(tx, tz));
                if !inside {
                    'g: for j in 0..=64 {
                        for i in 0..=64 {
                            let x = (tx * TILE) as f64 + (i as f64 / 64.0) * (TILE as f64 - 1.0) + 0.5;
                            let z = (tz * TILE) as f64 + (j as f64 / 64.0) * (TILE as f64 - 1.0) + 0.5;
                            let (la, lo) = self.lcc.mc_to_ll(x, z);
                            if self.area.contains(la, lo) {
                                inside = true;
                                break 'g;
                            }
                        }
                    }
                }
                if inside {
                    v.push((tx, tz));
                }
            }
        }
        v
    }
}

pub struct TileOut {
    pub h: Vec<f32>,
    pub k: Vec<u8>,
    pub min: f32,
    pub max: f32,
    pub counts: [u64; 6],
}

/// Separable minimum filter (radius r) over a w×h array (+inf = no value).
fn min_filter(a: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut t = vec![f32::INFINITY; w * h];
    t.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let src = &a[y * w..(y + 1) * w];
        for x in 0..w {
            let lo = x.saturating_sub(r);
            let hi = (x + r).min(w - 1);
            row[x] = src[lo..=hi].iter().copied().fold(f32::INFINITY, f32::min);
        }
    });
    let mut o = vec![f32::INFINITY; w * h];
    o.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let lo = y.saturating_sub(r);
        let hi = (y + r).min(h - 1);
        for yy in lo..=hi {
            let src = &t[yy * w..(yy + 1) * w];
            for x in 0..w {
                row[x] = row[x].min(src[x]);
            }
        }
    });
    o
}

pub fn process_tile(ctx: &Ctx, tx: i64, tz: i64) -> TileOut {
    let x0 = tx * TILE - HALO;
    let z0 = tz * TILE - HALO;
    let n = (TILE + 2 * HALO) as usize;
    let ll = LlGrid::new(&ctx.lcc, x0, z0, n as i64, n as i64);
    // lat/lon bounds of the window
    let (mut la0, mut la1, mut lo0, mut lo1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for j in 0..=16 {
        for i in 0..=16 {
            let (la, lo) = ll.get(x0 + (i * (n as i64 - 1)) / 16, z0 + (j * (n as i64 - 1)) / 16);
            la0 = la0.min(la);
            la1 = la1.max(la);
            lo0 = lo0.min(lo);
            lo1 = lo1.max(lo);
        }
    }
    let (la0, la1, lo0, lo1) = (la0 - 0.002, la1 + 0.002, lo0 - 0.002, lo1 + 0.002);
    let w1 = Window::new(&ctx.m1, la0, la1, lo0, lo1);
    let w5 = Window::new(&ctx.m5, la0, la1, lo0, lo1);
    let wl = Window::new(&ctx.wl, la0, la1, lo0, lo1);
    let hf = Window::new(&ctx.hf, la0, la1, lo0, lo1);
    // WA polygon masks, rasterized in bands
    let mut mask = vec![0u8; n * n];
    {
        let (fx0, fz0, fx1, fz1) = (x0 as f64, z0 as f64, (x0 + n as i64) as f64, (z0 + n as i64) as f64);
        let sel: Vec<&MPoly> = ctx.polys.iter().filter(|p| p.x1 >= fx0 && p.x0 <= fx1 && p.z1 >= fz0 && p.z0 <= fz1).collect();
        const BAND: usize = 256;
        mask.par_chunks_mut(BAND * n).enumerate().for_each(|(b, band)| {
            let rows = band.len() / n;
            let bz0 = fz0 + (b * BAND) as f64;
            let bz1 = bz0 + rows as f64;
            for p in &sel {
                if p.z1 < bz0 || p.z0 > bz1 {
                    continue;
                }
                let bit = if p.sea { M_SEA } else { M_INLAND };
                let rings: Vec<Vec<(f64, f64)>> = p.rings.iter().map(|r| r.iter().map(|&(x, z)| (x - fx0, z - bz0)).collect()).collect();
                raster::fill_polygon(&rings, n, rows, |r, c0, c1| {
                    for v in &mut band[r * n + c0..r * n + c1] {
                        *v |= bit;
                    }
                });
            }
        });
    }
    // 2nd meshes with 基本情報 (others are open sea)
    let (kr0, kr1) = (((46.0 - la1) * 12.0).floor() as i64, ((46.0 - la0) * 12.0).floor() as i64);
    let (kc0, kc1) = (((lo0 - 122.0) * 8.0).floor() as i64, ((lo1 - 122.0) * 8.0).floor() as i64);
    let kw = (kc1 - kc0 + 1) as usize;
    let mut ktab = vec![false; kw * (kr1 - kr0 + 1) as usize];
    for r in kr0..=kr1 {
        for c in kc0..=kc1 {
            ktab[(r - kr0) as usize * kw + (c - kc0) as usize] = ctx.kihon.contains(&(r, c));
        }
    }
    // pass 1: classify every pixel and take DEM elevations
    let mut hgt = vec![f32::NAN; n * n];
    let mut cls = vec![K_OUT; n * n];
    hgt.par_chunks_mut(n).zip(cls.par_chunks_mut(n)).enumerate().for_each(|(j, (hrow, krow))| {
        let z = z0 + j as i64;
        for i in 0..n {
            let x = x0 + i as i64;
            let (lat, lon) = ll.get(x, z);
            if !ctx.area.contains(lat, lon) {
                continue;
            }
            let m = mask[j * n + i];
            let (s, src) = match w1.sample(lat, lon) {
                Sample::None => (w5.sample(lat, lon), K_LAND5),
                s => (s, K_LAND1),
            };
            let water = |code_sea: bool| -> u8 {
                if m & M_SEA != 0 {
                    K_SEA
                } else if m & M_INLAND != 0 || !code_sea {
                    K_INLAND
                } else if matches!(wl.sample(lat, lon), Sample::Elev(_)) {
                    // coded as sea but not joined to the sea (lakes.rs): a river or lake
                    K_INLAND
                } else {
                    K_SEA
                }
            };
            let k = match s {
                // the DEM's sea floor (laser bathymetry along some coasts) inside
                // a sea polygon is sea, with its depth kept for the sea bed
                Sample::Elev(v) if v < 0.0 && m & M_SEA != 0 => {
                    hrow[i] = v.max(SEA_FLOOR);
                    K_SEA
                }
                Sample::Elev(v) => {
                    hrow[i] = v.max(SEA_FLOOR);
                    src
                }
                Sample::Sea => water(true),
                Sample::Inland => water(false),
                Sample::None => {
                    if m & M_SEA != 0 {
                        K_SEA
                    } else if m & M_INLAND != 0 {
                        K_INLAND
                    } else {
                        let kr = ((46.0 - lat) * 12.0).floor() as i64 - kr0;
                        let kc = ((lon - 122.0) * 8.0).floor() as i64 - kc0;
                        if ktab[kr as usize * kw + kc as usize] { K_HOLE } else { K_SEA }
                    }
                }
            };
            krow[i] = k;
            match k {
                // sea floor unknown here (the DEM's own sea bed is kept)
                K_SEA if !matches!(s, Sample::Elev(_)) => hrow[i] = SEA_FLOOR,
                K_INLAND => {
                    if let Sample::Elev(v) = wl.sample(lat, lon) {
                        hrow[i] = v.max(SEA_FLOOR);
                    }
                }
                K_HOLE => {
                    if let Sample::Elev(v) = hf.sample(lat, lon) {
                        hrow[i] = v.max(SEA_FLOOR);
                    }
                }
                _ => {}
            }
        }
    });
    // inland water without a 5m level (small 1m-only ponds/streams): lowest
    // land within 6 m
    let need: bool = cls.par_iter().zip(hgt.par_iter()).any(|(k, h)| *k == K_INLAND && h.is_nan());
    if need {
        let land: Vec<f32> = cls
            .par_iter()
            .zip(hgt.par_iter())
            .map(|(k, h)| if (*k == K_LAND1 || *k == K_LAND5) && !h.is_nan() { *h } else { f32::INFINITY })
            .collect();
        let mf = min_filter(&land, n, n, 6);
        hgt.par_iter_mut().zip(cls.par_iter()).zip(mf.par_iter()).for_each(|((h, k), m)| {
            if *k == K_INLAND && h.is_nan() && m.is_finite() {
                *h = *m;
            }
        });
    }
    // anything still unknown inside the area: interpolate from the window
    // (the sea counts as sea level, not as its -250 m floor, so a coastal
    // pixel without data does not become a pit)
    let need: bool = cls.par_iter().zip(hgt.par_iter()).any(|(k, h)| *k != K_OUT && h.is_nan());
    if need {
        let mut g: Vec<f32> = cls
            .par_iter()
            .zip(hgt.par_iter())
            .map(|(k, h)| match *k {
                K_OUT | K_INLAND => f32::NAN,
                K_SEA => 0.0,
                _ => *h,
            })
            .collect();
        raster::pull_push(&mut g, n, n);
        hgt.par_iter_mut().zip(cls.par_iter()).zip(g.par_iter()).for_each(|((h, k), v)| {
            if *k != K_OUT && h.is_nan() {
                *h = if v.is_nan() { 0.0 } else { *v };
            }
        });
    }
    // crop the halo
    let t = TILE as usize;
    let hh = HALO as usize;
    let mut h = vec![NODATA; t * t];
    let mut k = vec![K_OUT; t * t];
    h.par_chunks_mut(t).zip(k.par_chunks_mut(t)).enumerate().for_each(|(j, (hr, kr))| {
        let src = (j + hh) * n + hh;
        kr.copy_from_slice(&cls[src..src + t]);
        for i in 0..t {
            if kr[i] != K_OUT {
                hr[i] = hgt[src + i];
            }
        }
    });
    let (min, max, counts) = h
        .par_chunks(t)
        .zip(k.par_chunks(t))
        .map(|(hr, kr)| {
            let (mut mn, mut mx, mut c) = (f32::INFINITY, f32::NEG_INFINITY, [0u64; 6]);
            for i in 0..hr.len() {
                c[kr[i] as usize] += 1;
                if kr[i] != K_OUT {
                    mn = mn.min(hr[i]);
                    mx = mx.max(hr[i]);
                }
            }
            (mn, mx, c)
        })
        .reduce(
            || (f32::INFINITY, f32::NEG_INFINITY, [0u64; 6]),
            |a, b| {
                let mut c = a.2;
                for i in 0..6 {
                    c[i] += b.2[i];
                }
                (a.0.min(b.0), a.1.max(b.1), c)
            },
        );
    TileOut { h, k, min, max, counts }
}

/// Downsampled shaded thumbnail of a tile (factor 64 → 160×160 RGB).
fn thumbnail(o: &TileOut) -> Vec<u8> {
    const F: usize = 64;
    let t = TILE as usize;
    let s = t / F;
    let mut rgb = vec![0u8; s * s * 3];
    for y in 0..s {
        for x in 0..s {
            let i = (y * F + F / 2) * t + x * F + F / 2;
            let px = match o.k[i] {
                K_OUT => [0, 0, 0],
                K_SEA => [20, 40, 110],
                K_INLAND => [0, 190, 255],
                K_HOLE => [255, 40, 40],
                kk => {
                    let e = o.h[i].max(0.0);
                    let tt = (e / 3800.0).sqrt().min(1.0);
                    let base = if kk == K_LAND1 { [60.0, 140.0, 60.0] } else { [140.0, 140.0, 60.0] };
                    // simple NW hillshade from neighbouring pixels
                    let e2 = o.h[i - t - 1].max(0.0);
                    let sh = ((e - e2) * 0.05).clamp(-0.4, 0.4);
                    let f = |b: f32| ((b + (255.0 - b) * tt) * (1.0 + sh)).clamp(0.0, 255.0) as u8;
                    [f(base[0]), f(base[1]), f(base[2])]
                }
            };
            rgb[(y * s + x) * 3..(y * s + x) * 3 + 3].copy_from_slice(&px);
        }
    }
    rgb
}

pub fn run(ctx: &Ctx, out: &Path, only: Option<Vec<(i64, i64)>>, force: bool, concurrency: usize) -> Result<()> {
    std::fs::create_dir_all(out)?;
    let all = ctx.tile_list();
    let tiles = only.unwrap_or_else(|| all.clone());
    eprintln!("heightmap: {} tiles in area, processing {}", all.len(), tiles.len());
    let thumbs: Mutex<HashMap<(i64, i64), Vec<u8>>> = Mutex::new(HashMap::new());
    let rows: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let stats_path = out.join("tile_stats");
    std::fs::create_dir_all(&stats_path)?;
    let t0 = std::time::Instant::now();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let changed = std::sync::atomic::AtomicUsize::new(0);
    for chunk in tiles.chunks(concurrency.max(1)) {
        chunk.par_iter().try_for_each(|&(tx, tz)| -> Result<()> {
            let (x, z) = (tx * TILE, tz * TILE);
            let hp = out.join(height_name(x, z));
            let kp = out.join(class_name(x, z));
            let sp = stats_path.join(format!("{x}_{z}.txt"));
            if !force && hp.exists() && kp.exists() && sp.exists() {
                return Ok(());
            }
            let t = std::time::Instant::now();
            let o = process_tile(ctx, tx, tz);
            let tp = t.elapsed().as_secs_f64();
            if o.counts[0] == (TILE * TILE) as u64 {
                return Ok(()); // nothing inside
            }
            let c1 = tiff::write(&hp, tiff::Fmt::F32, TILE as usize, TILE as usize, tiff::f32_bytes(&o.h), x, z, Some("-9999"), 6)?;
            let c2 = tiff::write(&kp, tiff::Fmt::U8, TILE as usize, TILE as usize, &o.k, x, z, None, 6)?;
            if c1 || c2 {
                changed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            let line = format!(
                "{},{},{},{:.2},{:.2},{}",
                height_name(x, z),
                x,
                z,
                o.min,
                o.max,
                o.counts.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(",")
            );
            std::fs::write(&sp, &line)?;
            std::fs::write(stats_path.join(format!("{x}_{z}.thumb")), thumbnail(&o))?;
            rows.lock().unwrap().push(line);
            let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            eprintln!(
                "  tile X={x} Z={z}: min {:.1} max {:.1} (1m {:.0}%, 5m {:.0}%, filled {}) {:.1}s+{:.1}s  [{d}/{} {:.0}s]",
                o.min,
                o.max,
                o.counts[1] as f64 * 100.0 / (TILE * TILE) as f64,
                o.counts[2] as f64 * 100.0 / (TILE * TILE) as f64,
                o.counts[3],
                tp,
                t.elapsed().as_secs_f64() - tp,
                tiles.len(),
                t0.elapsed().as_secs_f64()
            );
            Ok(())
        })?;
    }
    drop(thumbs);
    drop(rows);
    eprintln!("heightmap: {} tiles written or changed", changed.into_inner());
    write_index(out)?;
    Ok(())
}

/// (Re)write tiles.csv, tiles_detail.csv and the overview from the per-tile stats.
pub fn write_index(out: &Path) -> Result<()> {
    let stats_path = out.join("tile_stats");
    let mut lines: Vec<(i64, i64, String)> = Vec::new();
    for e in std::fs::read_dir(&stats_path)? {
        let p = e?.path();
        if p.extension().is_some_and(|x| x == "txt") {
            let s = std::fs::read_to_string(&p)?;
            let c: Vec<&str> = s.trim().split(',').collect();
            lines.push((c[2].parse()?, c[1].parse()?, s.trim().to_string()));
        }
    }
    lines.sort();
    let mut f = std::io::BufWriter::new(std::fs::File::create(out.join("tiles.csv"))?);
    writeln!(f, "file,x,z,min_elevation_m,max_elevation_m")?;
    let mut d = std::io::BufWriter::new(std::fs::File::create(out.join("tiles_detail.csv"))?);
    writeln!(d, "file,x,z,min_elevation_m,max_elevation_m,px_{}", CLASS_NAMES.join(",px_"))?;
    let mut totals = [0u64; 6];
    for (_, _, l) in &lines {
        let c: Vec<&str> = l.split(',').collect();
        writeln!(f, "{}", c[..5].join(","))?;
        writeln!(d, "{l}")?;
        for i in 0..6 {
            totals[i] += c[5 + i].parse::<u64>()?;
        }
    }
    eprintln!("tiles.csv: {} tiles; pixel totals {:?}", lines.len(), CLASS_NAMES.iter().zip(totals).collect::<Vec<_>>());
    // overview mosaic of thumbnails
    if lines.is_empty() {
        return Ok(());
    }
    let s = (TILE / 64) as usize;
    let xs: Vec<i64> = lines.iter().map(|l| l.1 / TILE).collect();
    let zs: Vec<i64> = lines.iter().map(|l| l.0 / TILE).collect();
    let (tx0, tx1) = (*xs.iter().min().unwrap(), *xs.iter().max().unwrap());
    let (tz0, tz1) = (*zs.iter().min().unwrap(), *zs.iter().max().unwrap());
    let (w, h) = (((tx1 - tx0 + 1) as usize) * s, ((tz1 - tz0 + 1) as usize) * s);
    let mut rgb = vec![0u8; w * h * 3];
    for (z, x, _) in &lines {
        let p: PathBuf = stats_path.join(format!("{x}_{z}.thumb"));
        let Ok(th) = std::fs::read(&p) else { continue };
        let (ox, oz) = (((x / TILE) - tx0) as usize * s, ((z / TILE) - tz0) as usize * s);
        for y in 0..s {
            let dst = ((oz + y) * w + ox) * 3;
            rgb[dst..dst + s * 3].copy_from_slice(&th[y * s * 3..(y + 1) * s * 3]);
        }
    }
    raster::write_png(&out.join("overview.png"), w, h, &rgb)?;
    Ok(())
}
