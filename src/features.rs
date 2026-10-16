//! Per-tile feature rasters (same 10240² grid as the heightmap tiles):
//!   lc_*.tif     U16  land-cover class index (row of the mapping table + 1; 0 = none)
//!   bldg_*.tif   U32  building id (1-based row of bldg_*.bin; 0 = none)
//!   roof_*.tif   F32  LOD2 roof height above the building base (m), NaN elsewhere
//!   struct_*.tif U8   water structures (1 breakwater, 2 pier, 3 dam, 4 weir/sluice, 5 sabo dam)
//!   road_*.tif   U8   ground-level road surface type (see ROAD_* codes), 0 = none
//!   roadh_*.tif  F32  road surface height (m)
//!   rail_*.tif   U8   1 track (rail), 2 ballast, 3 slab track, 4 platform
//!   railh_*.tif  F32  rail/platform base height (m)
//!   tcov_*.tif   U8   measured-tree coverage: 1 planted forest with tree points, 2 LiDAR canopy
//!   lake_*.tif   F32  water depth (m below the surface): surveyed lake beds
//!                     (湖沼データ) and the sea floor (the DEM's sea bed, else
//!                     ETOPO 2022), NaN elsewhere
//!   rowdir_*.tif U8   direction of field rows (degrees + 1; 0 = none)
//!   canopy_*.tif U8   LiDAR canopy height of trees (0.2 m units, ≥ 4 m; 0 = none)
//! plus bldg_*.bin, the building table, and trees_*.bin, measured trees
//! ([lx u16, lz u16, height dm u16, species u8, source u8]). Files are rewritten only when their
//! content changes, so the world stage can rebuild only affected regions.

use crate::heightmap::{K_HOLE, K_INLAND, K_LAND1, K_LAND5, K_OUT, K_SEA, TILE};
use crate::landcover::Spec;
use crate::lulc::Lulc;
use crate::proj::{Lcc, LlGrid};
use crate::raster;
use crate::tiff::{self, Fmt};
use crate::vfeat::{TiledReader, VFeat};
use crate::world::TileSet;
use anyhow::Result;
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const HALO: i64 = 64;

// unified building usage
pub const U_HOUSE: u8 = 1;
pub const U_APARTMENT: u8 = 2;
pub const U_COMMERCIAL: u8 = 3;
pub const U_OFFICE: u8 = 4;
pub const U_FACTORY: u8 = 5;
pub const U_WAREHOUSE: u8 = 6;
pub const U_PUBLIC: u8 = 7;
pub const U_SCHOOL: u8 = 8;
pub const U_HOSPITAL: u8 = 9;
pub const U_HOTEL: u8 = 10;
pub const U_RELIGIOUS: u8 = 11;
pub const U_STATION: u8 = 12;
pub const U_GREENHOUSE: u8 = 13;
pub const U_SHED: u8 = 14;
pub const U_UNKNOWN: u8 = 15;
pub const USAGE_NAMES: [&str; 16] = [
    "", "住宅", "共同住宅", "商業", "業務", "工場", "倉庫・物流", "官公庁", "学校", "病院・福祉", "宿泊", "寺社", "駅・交通", "温室", "車庫・小屋・無壁舎", "不明",
];

// roof kinds
pub const R_FLAT: u8 = 0;
pub const R_GABLE: u8 = 1;
pub const R_LOD2: u8 = 3;
pub const R_OPEN: u8 = 4; // 無壁舎: roof on pillars

#[derive(Clone, Copy, Debug, Default)]
pub struct BldRec {
    pub src: u8, // 1 PLATEAU, 2 OSM, 3 基盤地図情報
    pub usage: u8,
    pub roof: u8,
    pub ridge_x: u8, // 1 = ridge runs along X
    pub storeys: u8,
    pub base: f32,   // floor elevation (m)
    pub height: f32, // m above base (eaves for gable roofs, top otherwise)
    pub seed: u32,
}

impl BldRec {
    const SIZE: usize = 20;
    fn write(&self, b: &mut Vec<u8>) {
        b.extend_from_slice(&[self.src, self.usage, self.roof, self.ridge_x, self.storeys, 0, 0, 0]);
        b.extend_from_slice(&self.base.to_le_bytes());
        b.extend_from_slice(&self.height.to_le_bytes());
        b.extend_from_slice(&self.seed.to_le_bytes());
    }
    fn read(d: &[u8]) -> BldRec {
        BldRec {
            src: d[0],
            usage: d[1],
            roof: d[2],
            ridge_x: d[3],
            storeys: d[4],
            base: f32::from_le_bytes(d[8..12].try_into().unwrap()),
            height: f32::from_le_bytes(d[12..16].try_into().unwrap()),
            seed: u32::from_le_bytes(d[16..20].try_into().unwrap()),
        }
    }
}

pub fn read_bldg_table(path: &Path) -> Vec<BldRec> {
    let Ok(d) = std::fs::read(path) else { return vec![] };
    d.chunks_exact(BldRec::SIZE).map(BldRec::read).collect()
}

pub fn layer_name(layer: &str, x: i64, z: i64, ext: &str) -> String {
    format!("{layer}_X{x:+07}_Z{z:+07}.{ext}")
}

pub struct Ctx {
    pub work: PathBuf,
    bathy: Vec<crate::bathy::Grid>,
    sea: Option<crate::bathy::Sea>,
    pub lcc: Lcc,
    pub ts: TileSet,
    pub out: PathBuf,
    pub lulc: Lulc,
    /// key -> index (1-based) into `classes`
    pub class_index: BTreeMap<String, u16>,
    pub classes: Vec<Spec>,
    veg: TiledReader,
    osm_area: TiledReader,
    osm_road: TiledReader,
    osm_rail: TiledReader,
    osm_platform: TiledReader,
    osm_struct: TiledReader,
    rdedg: TiledReader,
    rdcompt: TiledReader,
    net_struct: TiledReader,
    plateau: TiledReader,
    kihon_bldg: TiledReader,
    wstra: TiledReader,
    wstrl: TiledReader,
}

impl Ctx {
    pub fn new(work: &Path, hm_dir: &Path, out: &Path, data: &Path, lcmap: &[Spec], area: &crate::area::Area) -> Result<Self> {
        let classes: Vec<Spec> = lcmap.to_vec();
        let class_index = classes.iter().enumerate().map(|(i, s)| (s.key.clone(), i as u16 + 1)).collect();
        let lulc = Lulc::load(
            &data.join("高解像度土地利用土地被覆図").join("2024JPN_v25.04.zip"),
            area.lat_min.floor() as i32,
            area.lat_max.ceil() as i32,
            area.lon_min.floor() as i32,
            area.lon_max.ceil() as i32,
        )?;
        eprintln!("features: JAXA land cover tiles loaded: {}", lulc.n);
        Ok(Ctx {
            work: work.to_path_buf(),
            bathy: crate::bathy::load(&data.join("湖沼データ"))?,
            sea: crate::bathy::Sea::load(&data.join("ETOPO2022"), area.lat_min - 0.05, area.lat_max + 0.05, area.lon_min - 0.05, area.lon_max + 0.05)?,
            lcc: Lcc::new(),
            ts: TileSet::new(hm_dir),
            out: out.to_path_buf(),
            lulc,
            class_index,
            classes,
            veg: TiledReader::open(&work.join("vec_veg.bin"))?,
            osm_area: TiledReader::open(&work.join("vec_osm_area.bin"))?,
            osm_road: TiledReader::open(&work.join("vec_osm_road.bin"))?,
            osm_rail: TiledReader::open(&work.join("vec_osm_rail.bin"))?,
            osm_platform: TiledReader::open(&work.join("vec_osm_platform.bin"))?,
            osm_struct: TiledReader::open(&work.join("vec_osm_struct.bin"))?,
            rdedg: TiledReader::open(&work.join("vec_kihon_rdedg.bin"))?,
            rdcompt: TiledReader::open(&work.join("vec_kihon_rdcompt.bin"))?,
            net_struct: TiledReader::open(&work.join("vec_net_struct.bin"))?,
            plateau: TiledReader::open(&work.join("vec_plateau_bldg.bin"))?,
            kihon_bldg: TiledReader::open(&work.join("vec_kihon_blda.bin"))?,
            wstra: TiledReader::open(&work.join("vec_kihon_wstra.bin"))?,
            wstrl: TiledReader::open(&work.join("vec_kihon_wstrl.bin"))?,
        })
    }

    fn idx(&self, key: &str) -> u16 {
        self.class_index.get(key).copied().unwrap_or(0)
    }
}

/// Window raster helper: (x0, z0) is the MC coordinate of pixel (0,0).
pub struct Win {
    pub x0: i64,
    pub z0: i64,
    pub n: usize,
}

impl Win {
    #[inline]
    pub fn px(&self, p: [f64; 2]) -> (f64, f64) {
        (p[0] - self.x0 as f64, p[1] - self.z0 as f64)
    }
    pub fn rings(&self, parts: &[Vec<[f64; 2]>]) -> Vec<Vec<(f64, f64)>> {
        parts.iter().map(|r| r.iter().map(|&p| self.px(p)).collect()).collect()
    }
    /// Fill polygon rings; calls f(index) for each covered pixel.
    pub fn fill(&self, parts: &[Vec<[f64; 2]>], mut f: impl FnMut(usize)) {
        let n = self.n;
        raster::fill_polygon(&self.rings(parts), n, n, |r, c0, c1| {
            for c in c0..c1 {
                f(r * n + c);
            }
        });
    }
    /// Rasterize polylines (1 px wide, 8-connected DDA); f(index).
    pub fn lines(&self, parts: &[Vec<[f64; 2]>], mut f: impl FnMut(usize)) {
        let n = self.n as i64;
        for p in parts {
            for w in p.windows(2) {
                let (ax, az) = self.px(w[0]);
                let (bx, bz) = self.px(w[1]);
                let steps = ((bx - ax).abs().max((bz - az).abs()).ceil() as i64).max(1);
                for s in 0..=steps {
                    let t = s as f64 / steps as f64;
                    let x = (ax + (bx - ax) * t).floor() as i64;
                    let z = (az + (bz - az) * t).floor() as i64;
                    if x >= 0 && z >= 0 && x < n && z < n {
                        f((z * n + x) as usize);
                    }
                }
            }
        }
    }
}

fn hash(a: u64, b: u64) -> u32 {
    let mut h = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 31;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 29;
    (h >> 32) as u32
}

fn tag<'a>(text: &'a str, k: &str) -> Option<&'a str> {
    text.split(';').find_map(|kv| kv.split_once('=').filter(|(kk, _)| *kk == k).map(|(_, v)| v))
}

/// Unified usage from a PLATEAU bldg:usage code.
fn plateau_usage(code: f32) -> u8 {
    match code as i32 {
        411 | 413 | 415 => U_HOUSE,
        412 | 414 => U_APARTMENT,
        401 => U_OFFICE,
        402 | 404 => U_COMMERCIAL,
        403 => U_HOTEL,
        421 | 453 => U_PUBLIC,
        422 => U_SCHOOL,
        431 => U_WAREHOUSE,
        441 | 452 => U_FACTORY,
        451 => U_SHED,
        _ => U_UNKNOWN,
    }
}

pub struct TileOut {
    pub lc: Vec<u16>,
    pub bldg: Vec<u32>,
    pub roof: Vec<f32>,
    pub strct: Vec<u8>,
    pub road: Vec<u8>,
    pub roadh: Vec<f32>,
    pub rail: Vec<u8>,
    pub railh: Vec<f32>,
    pub tcov: Vec<u8>,
    pub lake: Vec<f32>,
    pub rowdir: Vec<u8>,
    pub canopy: Vec<u8>,
    pub trees: Vec<u8>,
    pub ntrees: usize,
    pub table: Vec<BldRec>,
    /// PLATEAU buildings, 基盤地図情報 buildings, road ways, rail ways
    pub stats: [usize; 4],
}

pub fn process_tile(ctx: &Ctx, tx: i64, tz: i64) -> Result<TileOut> {
    let t = TILE as usize;
    let h = HALO;
    let w = Win { x0: tx * TILE - h, z0: tz * TILE - h, n: (TILE + 2 * h) as usize };
    let n = w.n;
    let (hgt, cls) = ctx.ts.window(w.x0, w.z0, n, n)?;
    let ll = LlGrid::new(&ctx.lcc, w.x0, w.z0, n as i64, n as i64);
    // ---- land cover: JAXA base, vegetation map on top, OSM areas on top of that
    let jaxa_idx: Vec<u16> = (0..=15u8).map(|c| ctx.idx(&format!("jaxa:{c}"))).collect();
    let mut lc = vec![0u16; n * n];
    let mut jx = vec![0u8; n * n];
    lc.par_chunks_mut(n).zip(jx.par_chunks_mut(n)).enumerate().for_each(|(j, (row, jrow))| {
        for i in 0..n {
            let (la, lo) = ll.get(w.x0 + i as i64, w.z0 + j as i64);
            let c = ctx.lulc.at(la, lo);
            jrow[i] = c;
            row[i] = jaxa_idx[c.min(15) as usize];
        }
    });
    let tile_veg = ctx.veg.tile(tx, tz);
    for f in &tile_veg {
        let i = ctx.idx(&format!("veg:{}", f.text));
        if i == 0 {
            continue;
        }
        w.fill(&f.parts, |p| lc[p] = i);
    }
    // JAXA refinements: solar panels / greenhouses always; forest patches inside urban classes
    let cat = |i: u16| -> &str { if i == 0 { "" } else { &ctx.classes[i as usize - 1].category } };
    for p in 0..n * n {
        let j = jx[p];
        if j == 12 || j == 14 {
            lc[p] = jaxa_idx[j as usize];
        } else if matches!(j, 6..=9 | 11) && matches!(cat(lc[p]), "urban" | "residential" | "industrial") {
            lc[p] = jaxa_idx[j as usize];
        }
    }
    // OSM land use: specific classes override, broad zones only on generic land
    let osm_keys: Vec<(&str, &str, u16)> = crate::landcover::osm_defaults()
        .iter()
        .filter_map(|s| {
            let kv = s.key.strip_prefix("osm:")?;
            let (k, v) = kv.split_once('=')?;
            let i = ctx.idx(&s.key);
            Some((Box::leak(k.to_string().into_boxed_str()) as &str, Box::leak(v.to_string().into_boxed_str()) as &str, i))
        })
        .collect();
    let broad = ["industrial", "commercial", "retail", "military", "railway", "farmland"];
    let mut areas: Vec<(f64, u16, bool, &VFeat)> = Vec::new();
    let tile_area = ctx.osm_area.tile(tx, tz);
    for f in &tile_area {
        if let Some(&(_, v, i)) = osm_keys.iter().find(|(k, v, _)| tag(&f.text, k) == Some(v)) {
            let a = crate::ingest::ring_area(&f.parts[0]).abs();
            areas.push((a, i, broad.contains(&v), f));
        }
    }
    // large polygons first so smaller (more specific) ones end up on top
    areas.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    // Farm parcels mapped in OSM without a crop: tea where the parcel overlaps
    // the vegetation map's tea fields, where tea dominates the farmland around
    // it (牧之原・川根 etc. — the map generalises parcels into weeds/bare), or
    // where the LiDAR canopy shows knee-high hedges over most of the parcel.
    let tea = ctx.idx("veg:570201");
    let lc_pre = lc.clone();
    const CS: usize = 32;
    let ncs = n / CS + 1;
    let (mut tea_c, mut farm_c) = (vec![0u32; ncs * ncs], vec![0u32; ncs * ncs]);
    for p in 0..n * n {
        let k = (p / n / CS) * ncs + (p % n) / CS;
        match cat(lc_pre[p]) {
            "tea" => {
                tea_c[k] += 1;
                farm_c[k] += 1;
            }
            "farm" | "grass" | "bare" | "orchard" => farm_c[k] += 1,
            _ => {}
        }
    }
    let chm_core = crate::trees::read_chm(&ctx.work, tx, tz);
    let low_hedge = |p: usize| -> Option<bool> {
        let (chm, cov) = chm_core.as_ref()?;
        let (x, z) = ((p % n) as i64 - h, (p / n) as i64 - h);
        if x < 0 || z < 0 || x >= TILE || z >= TILE {
            return None;
        }
        let q = z as usize * t + x as usize;
        (cov[q] != 0).then(|| (2..=9).contains(&chm[q]))
    };
    for (_, i, is_broad, f) in &areas {
        let mut i = *i;
        let mut pix = Vec::new();
        w.fill(&f.parts, |p| pix.push(p));
        if pix.is_empty() {
            continue;
        }
        if tea > 0 && tag(&f.text, "landuse") == Some("farmland") && tag(&f.text, "crop").is_none() {
            let overlap = pix.iter().filter(|&&p| lc_pre[p] == tea).count() as f32 / pix.len() as f32;
            let c = pix[pix.len() / 2];
            let (cx, cz) = ((c % n) / CS, (c / n) / CS);
            let (mut tt, mut ff) = (0u32, 0u32);
            for kz in cz.saturating_sub(6)..(cz + 7).min(ncs) {
                for kx in cx.saturating_sub(6)..(cx + 7).min(ncs) {
                    tt += tea_c[kz * ncs + kx];
                    ff += farm_c[kz * ncs + kx];
                }
            }
            let (mut seen, mut hedge) = (0usize, 0usize);
            for &p in &pix {
                if let Some(v) = low_hedge(p) {
                    seen += 1;
                    hedge += v as usize;
                }
            }
            let lidar_tea = seen * 2 > pix.len() && hedge * 10 > seen * 4;
            if overlap >= 0.2 || (ff > 2000 && tt * 20 > ff * 7) || lidar_tea {
                i = tea;
            }
        }
        if *is_broad && i != tea {
            for p in pix {
                if matches!(cat(lc[p]), "urban" | "grass" | "bare" | "residential" | "") {
                    lc[p] = i;
                }
            }
        } else {
            for p in pix {
                if cat(lc[p]) != "water" {
                    lc[p] = i;
                }
            }
        }
    }
    // row direction of each mapped parcel: along its long side (minimum-area
    // bounding rectangle of the outline)
    let mut parcel_dir = vec![0u8; n * n];
    for (_, _, _, f) in &areas {
        let v = ["landuse", "crop", "building", "plant:source", "generator:source"].iter().find_map(|k| tag(&f.text, k)).unwrap_or("");
        if !matches!(v, "farmland" | "orchard" | "vineyard" | "greenhouse_horticulture" | "greenhouse" | "tea" | "solar" | "plant_nursery") {
            continue;
        }
        let Some(a) = long_axis_deg(&f.parts[0]) else { continue };
        let d = (a.round() as u16 % 180) as u8 + 1;
        w.fill(&f.parts, |p| parcel_dir[p] = d);
    }
    // runways and taxiways mapped as centre lines (closed areas come with the areas above)
    let osm_struct = ctx.osm_struct.tile(tx, tz);
    let mut runways: Vec<(Vec<[f64; 2]>, Vec<f64>, f64)> = Vec::new(); // densified px line, arc, half width
    for f in &osm_struct {
        let kind = tag(&f.text, "aeroway");
        let (Some(k @ ("runway" | "taxiway")), false) = (kind, f.area) else { continue };
        let width: f64 = tag(&f.text, "width").and_then(|v| v.trim_end_matches(" m").parse().ok()).unwrap_or(if k == "runway" { 45.0 } else { 23.0 });
        let cls_i = ctx.idx(if k == "runway" { "osm:aeroway=runway" } else { "osm:aeroway=taxiway" });
        let px: Vec<[f64; 2]> = f.parts[0].iter().map(|q| [q[0] - w.x0 as f64, q[1] - w.z0 as f64]).collect();
        let (pts, arc) = crate::roads::densify(&px, 2.0);
        for k2 in 0..pts.len().saturating_sub(1) {
            crate::roads::segment_pixels(n, pts[k2], pts[k2 + 1], width / 2.0, |p, _, _| {
                if cls[p] != K_SEA && cls[p] != K_INLAND && cls[p] != K_OUT {
                    lc[p] = cls_i;
                }
            });
        }
        if k == "runway" {
            runways.push((pts, arc, width / 2.0));
        }
    }
    // water from the terrain classes
    let water = ctx.idx("veg:580600");
    for p in 0..n * n {
        if cls[p] == K_SEA || cls[p] == K_INLAND {
            lc[p] = water;
        } else if cls[p] == K_OUT {
            lc[p] = 0;
        }
    }
    // ---- buildings: PLATEAU, then 基盤地図情報 where nothing overlaps (OSM
    // buildings are not used, see README: licence)
    let mut bldg = vec![0u32; n * n];
    let mut roof = vec![f32::NAN; n * n];
    let mut table: Vec<BldRec> = Vec::new();
    let mut stats = [0usize; 4];
    let ground_stats = |pix: &[usize]| -> Option<(f32, f32)> {
        let mut v: Vec<f32> = pix.iter().filter(|&&p| matches!(cls[p], K_LAND1 | K_LAND5 | K_HOLE)).map(|&p| hgt[p]).collect();
        if v.is_empty() {
            return None;
        }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        Some((v[v.len() / 2], v[0]))
    };
    let place = |pix: Vec<usize>, rec: BldRec, bldg: &mut Vec<u32>, table: &mut Vec<BldRec>| -> Option<u32> {
        if pix.is_empty() {
            return None;
        }
        table.push(rec);
        let id = table.len() as u32;
        for p in pix {
            bldg[p] = id;
        }
        Some(id)
    };
    // footprints that are mostly already taken are skipped (duplicates across sources)
    let free_pixels = |parts: &[Vec<[f64; 2]>], bldg: &Vec<u32>| -> (Vec<usize>, usize) {
        let mut all = Vec::new();
        w.fill(parts, |p| all.push(p));
        let total = all.len();
        let free: Vec<usize> = all.into_iter().filter(|&p| bldg[p] == 0 && cls[p] != K_OUT && cls[p] != K_SEA).collect();
        (free, total)
    };
    let long_axis_x = |parts: &[Vec<[f64; 2]>]| -> u8 {
        let r = &parts[0];
        let (mut x0, mut x1, mut z0, mut z1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for p in r {
            x0 = x0.min(p[0]);
            x1 = x1.max(p[0]);
            z0 = z0.min(p[1]);
            z1 = z1.max(p[1]);
        }
        ((x1 - x0) >= (z1 - z0)) as u8
    };
    for f in ctx.plateau.tile(tx, tz) {
        // footprint rings first (outer and courtyards), then the LOD2 roof polygons
        let nfoot = (f.class as usize).clamp(1, f.parts.len());
        let (free, total) = free_pixels(&f.parts[..nfoot], &bldg);
        if total == 0 || free.len() * 10 < total * 5 {
            continue;
        }
        let Some((base, _)) = ground_stats(&free) else { continue };
        let lod2 = f.flags & crate::plateau::F_LOD2 != 0;
        let height = if f.nums[0] > 0.5 { f.nums[0] } else { 3.0 * f.nums[1].max(1.0) };
        let storeys = if f.nums[1] >= 1.0 { f.nums[1] } else { (height / 3.2).round().max(1.0) };
        let seed = hash(f.id, (f.parts[0][0][0] * 10.0) as u64 ^ (f.parts[0][0][1] * 10.0) as u64);
        let rec = BldRec {
            src: 1,
            usage: plateau_usage(f.nums[2]),
            roof: if lod2 { R_LOD2 } else { R_FLAT },
            ridge_x: long_axis_x(&f.parts),
            storeys: storeys.min(250.0) as u8,
            base,
            height,
            seed,
        };
        if let Some(id) = place(free, rec, &mut bldg, &mut table) {
            stats[0] += 1;
            if lod2 {
                // z-buffer the roof polygons: height above the PLATEAU ground level
                let gz = f.nums[3];
                for (part, zs) in f.parts.iter().zip(&f.zs).skip(nfoot) {
                    let pts: Vec<(f64, f64, f32)> = part.iter().zip(zs).map(|(&p, &z)| {
                        let (x, y) = w.px(p);
                        (x, y, z - gz)
                    }).collect();
                    raster_roof(&pts, n, &bldg, id, &mut roof);
                }
            }
        }
    }
    for f in ctx.kihon_bldg.tile(tx, tz) {
        let (free, total) = free_pixels(&f.parts, &bldg);
        if total == 0 || free.len() * 10 < total * 7 {
            continue;
        }
        let Some((base, _)) = ground_stats(&free) else { continue };
        let (usage, roof_kind) = match f.class {
            3 | 4 => (U_SHED, R_OPEN),
            _ => (U_UNKNOWN, R_FLAT),
        };
        let rec = BldRec {
            src: 3,
            usage,
            roof: roof_kind,
            ridge_x: long_axis_x(&f.parts),
            storeys: 0,
            base,
            height: 0.0,
            seed: hash(f.parts[0][0][0].to_bits(), f.parts[0][0][1].to_bits()),
        };
        let rc = f.class == 2 || f.class == 4; // 堅ろう
        if let Some(id) = place(free, rec, &mut bldg, &mut table) {
            stats[2] += 1;
            // remember 堅ろう in the roof field for the heuristics below
            if rc {
                table[id as usize - 1].ridge_x |= 2;
            }
        }
    }
    // fill in usage / height / roof for buildings without that information
    let mut area_px = vec![0u32; table.len() + 1];
    let mut lc_vote: Vec<BTreeMap<&str, u32>> = vec![BTreeMap::new(); table.len() + 1];
    for p in 0..n * n {
        let b = bldg[p] as usize;
        if b > 0 {
            area_px[b] += 1;
            *lc_vote[b].entry(cat(lc[p])).or_default() += 1;
        }
    }
    for (i, r) in table.iter_mut().enumerate() {
        let a = area_px[i + 1] as f32;
        let zone = lc_vote[i + 1].iter().max_by_key(|(_, v)| **v).map(|(k, _)| *k).unwrap_or("");
        let rc = r.ridge_x & 2 != 0;
        r.ridge_x &= 1;
        if r.usage == U_UNKNOWN {
            r.usage = match zone {
                "industrial" => if a > 1500.0 { U_WAREHOUSE } else { U_FACTORY },
                "urban" if rc => if a > 600.0 { U_COMMERCIAL } else { U_APARTMENT },
                "urban" => if a > 400.0 { U_COMMERCIAL } else { U_HOUSE },
                "farm" | "paddy" | "tea" | "orchard" => if a > 250.0 { U_SHED } else { U_HOUSE },
                _ => if rc { if a > 600.0 { U_OFFICE } else { U_APARTMENT } } else if a > 500.0 { U_FACTORY } else { U_HOUSE },
            };
        }
        if r.height <= 0.0 {
            let st: f32 = match r.usage {
                U_HOUSE => 2.0,
                U_SHED => 1.0,
                U_GREENHOUSE => 1.0,
                U_FACTORY | U_WAREHOUSE => 2.0,
                U_APARTMENT => if zone == "urban" { 5.0 } else { 3.0 },
                U_COMMERCIAL | U_OFFICE | U_HOTEL => if a > 2000.0 { 7.0 } else if a > 600.0 { 5.0 } else { 3.0 },
                U_SCHOOL | U_HOSPITAL | U_PUBLIC => 4.0,
                _ => 2.0,
            };
            r.storeys = st as u8;
            r.height = match r.usage {
                U_FACTORY | U_WAREHOUSE => 9.0,
                U_SHED => 3.5,
                _ => st * 3.1 + 0.4,
            };
        }
        if r.storeys == 0 {
            r.storeys = (r.height / 3.2).round().clamp(1.0, 250.0) as u8;
        }
        // detached houses and small sheds get pitched roofs
        if r.roof == R_FLAT && r.src != 1 && (r.usage == U_HOUSE || (r.usage == U_SHED && a < 120.0)) && a < 400.0 {
            r.roof = R_GABLE;
        }
        if r.roof == R_FLAT && r.src == 1 && r.usage == U_HOUSE && r.storeys <= 3 && a < 300.0 {
            r.roof = R_GABLE;
        }
    }
    // ---- water structures
    let mut strct = vec![0u8; n * n];
    let scode = |ty: &str| -> u8 {
        match ty.split('|').next().unwrap_or("") {
            "防波堤" | "不透過水制" | "透過水制" => 1,
            "桟橋（鉄，コンクリート）" | "敷石斜坂" => 2,
            "ダム" => 3,
            "せき" | "水門" => 4,
            "砂防ダム" => 5,
            _ => 0,
        }
    };
    for f in ctx.wstra.tile(tx, tz) {
        let c = scode(&f.text);
        if c > 0 {
            w.fill(&f.parts, |p| strct[p] = c);
        }
    }
    for f in ctx.wstrl.tile(tx, tz) {
        let c = scode(&f.text);
        if c > 0 {
            w.lines(&f.parts, |p| strct[p] = strct[p].max(c));
        }
    }
    // OSM piers and breakwaters where 基盤地図情報 has none (quays of ports, jetties)
    for f in &osm_struct {
        let c = match tag(&f.text, "man_made") {
            Some("pier") => 2,
            Some("breakwater" | "groyne") => 1,
            _ => continue,
        };
        if f.area {
            w.fill(&f.parts, |p| {
                if strct[p] == 0 {
                    strct[p] = c;
                }
            });
        } else {
            for part in &f.parts {
                let px: Vec<[f64; 2]> = part.iter().map(|q| [q[0] - w.x0 as f64, q[1] - w.z0 as f64]).collect();
                let (pts, _) = crate::roads::densify(&px, 2.0);
                for k2 in 0..pts.len().saturating_sub(1) {
                    crate::roads::segment_pixels(n, pts[k2], pts[k2 + 1], 2.0, |p, _, _| {
                        if strct[p] == 0 {
                            strct[p] = c;
                        }
                    });
                }
            }
        }
    }
    // ---- roads, railways, platforms
    let (roads, rails, platforms) = (ctx.osm_road.tile(tx, tz), ctx.osm_rail.tile(tx, tz), ctx.osm_platform.tile(tx, tz));
    let (rdedg, rdcompt, structs) = (ctx.rdedg.tile(tx, tz), ctx.rdcompt.tile(tx, tz), ctx.net_struct.tile(tx, tz));
    let net = crate::roads::rasterize(
        &w,
        &hgt,
        &cls,
        &crate::roads::Inputs { roads: &roads, rails: &rails, platforms: &platforms, rdedg: &rdedg, rdcompt: &rdcompt, structs: &structs },
    );
    // runway markings: centre-line dashes, side stripes, threshold bars
    let mut net = net;
    for (pts, arc, hw) in &runways {
        let len = *arc.last().unwrap_or(&0.0);
        for k in 0..pts.len().saturating_sub(1) {
            crate::roads::segment_pixels(n, pts[k], pts[k + 1], *hw, |p, u, t| {
                // perpendicular projections only (a segment's round cap would
                // report the distance to its end point, not the lateral offset)
                if cls[p] == K_OUT || t <= 0.0 || t >= 1.0 {
                    return;
                }
                let sa = arc[k] + (arc[k + 1] - arc[k]) * t;
                let au = u.abs();
                let end = sa < 60.0 || len - sa < 60.0;
                let mark = (au < 0.5 && sa.rem_euclid(50.0) < 30.0 && !end)
                    || (au >= hw - 2.0 && au < hw - 1.0)
                    || (end && (sa < 50.0 || len - sa < 50.0) && (sa < 6.0 || len - sa < 6.0 || au < hw - 4.0 && (au.floor() as i64).rem_euclid(4) < 2));
                if mark {
                    net.road[p] = crate::infra::RD_MARK_WHITE;
                    net.roadh[p] = hgt[p];
                }
            });
        }
    }
    stats[1] = stats[2];
    stats[2] = net.stats[0];
    stats[3] = net.stats[2];
    // crop the halo
    let h = HALO as usize;
    let crop = |v: &[u8]| -> Vec<u8> { (0..t).flat_map(|j| v[(j + h) * n + h..(j + h) * n + h + t].iter().copied()).collect() };
    let cropf = |v: &[f32]| -> Vec<f32> { (0..t).flat_map(|j| v[(j + h) * n + h..(j + h) * n + h + t].iter().copied()).collect() };
    let mut out = TileOut {
        lc: vec![0; t * t],
        bldg: vec![0; t * t],
        roof: cropf(&roof),
        strct: crop(&strct),
        road: crop(&net.road),
        roadh: cropf(&net.roadh),
        rail: crop(&net.rail),
        railh: cropf(&net.railh),
        tcov: Vec::new(),
        lake: vec![f32::NAN; t * t],
        rowdir: Vec::new(),
        canopy: Vec::new(),
        trees: Vec::new(),
        ntrees: 0,
        table,
        stats,
    };
    for j in 0..t {
        let src = (j + h) * n + h;
        out.lc[j * t..(j + 1) * t].copy_from_slice(&lc[src..src + t]);
        out.bldg[j * t..(j + 1) * t].copy_from_slice(&bldg[src..src + t]);
    }
    // ---- surveyed lake beds
    let (mx0, mz0) = (tx * TILE, tz * TILE);
    for g in &ctx.bathy {
        let (bx0, bz0, bx1, bz1) = g.bbox;
        let (xs, xe) = ((bx0.floor() as i64 - mx0).max(0), (bx1.ceil() as i64 - mx0).min(TILE));
        let (zs, ze) = ((bz0.floor() as i64 - mz0).max(0), (bz1.ceil() as i64 - mz0).min(TILE));
        for z in zs..ze {
            for x in xs..xe {
                let wi = (z as usize + h) * n + x as usize + h;
                if cls[wi] != K_INLAND {
                    continue;
                }
                let d = g.sample((mx0 + x) as f64 + 0.5, (mz0 + z) as f64 + 0.5);
                if d.is_finite() {
                    out.lake[z as usize * t + x as usize] = d;
                }
            }
        }
    }
    // ---- sea floor: where the DEM has the sea bed (laser bathymetry along some
    // coasts) its depth; elsewhere ETOPO 2022 below mean sea level, eased in
    // over the first COAST m from the shore (the 15″ grid does not resolve the
    // coastline) and blended into the DEM sea bed over BLEND m around it
    if cls.iter().any(|&c| c == K_SEA) {
        const COAST: f32 = 300.0;
        const BLEND: f32 = 150.0;
        let hal = 320i64;
        let w2 = (TILE + 2 * hal) as usize;
        let (h2, k2) = ctx.ts.window(mx0 - hal, mz0 - hal, w2, w2)?;
        let land: Vec<bool> = k2.iter().map(|&c| matches!(c, K_LAND1 | K_LAND5 | K_HOLE)).collect();
        let dist = crate::rgen::distance(&land, w2, w2, COAST + 10.0);
        drop(land);
        let dem: Vec<f32> = k2.iter().zip(&h2).map(|(&c, &v)| if c == K_SEA && v > crate::heightmap::SEA_FLOOR + 0.5 { (-v).max(1.0) } else { f32::NAN }).collect();
        drop(h2);
        let has_dem = dem.iter().any(|v| v.is_finite());
        let (ddist, dval) = if has_dem { nearest_value(&dem, w2, w2, BLEND + 10.0) } else { (Vec::new(), Vec::new()) };
        let ll = LlGrid::new(&ctx.lcc, mx0, mz0, TILE, TILE);
        let hal = hal as usize;
        let smooth = |s: f32| s * s * (3.0 - 2.0 * s);
        out.lake.par_chunks_mut(t).enumerate().for_each(|(z, row)| {
            for x in 0..t {
                let wi = (z + hal) * w2 + x + hal;
                if k2[wi] != K_SEA {
                    continue;
                }
                if has_dem && dem[wi].is_finite() {
                    row[x] = dem[wi];
                    continue;
                }
                let (la, lo) = ll.get(mx0 + x as i64, mz0 + z as i64);
                let e = ctx.sea.as_ref().map(|s| s.at(la, lo)).unwrap_or(f32::NAN);
                let mut d = if e.is_finite() { 1.0 + ((-e).max(1.0) - 1.0) * smooth((dist[wi] / COAST).min(1.0)) } else { f32::NAN };
                if has_dem && ddist[wi] < BLEND {
                    let far = if d.is_finite() { d } else { dval[wi] };
                    d = dval[wi] + (far - dval[wi]) * smooth(ddist[wi] / BLEND);
                }
                if d.is_finite() {
                    row[x] = d.max(1.0);
                }
            }
        });
    }
    // ---- row direction of fields, tea, orchards, greenhouses, solar farms:
    // a mapped parcel's long side; elsewhere the prevailing direction of the
    // roads around (farm plots are laid out on their road grid)
    {
        let patterned: Vec<bool> = (0..ctx.classes.len() + 1)
            .map(|c| c > 0 && !matches!(ctx.classes[c - 1].pattern.as_str(), "" | "none"))
            .collect();
        // per 32 m cell: histogram of road directions modulo 90° (3° bins)
        let nc = n / CS + 1;
        let mut hist = vec![[0u16; 30]; nc * nc];
        for p in 0..n * n {
            let d = net.rdir[p];
            if d != 0 {
                let a = ((d - 1) as usize % 90) / 3;
                let k = (p / n / CS) * nc + (p % n) / CS;
                hist[k][a] = hist[k][a].saturating_add(1);
            }
        }
        let mut cell_dir = vec![0u8; nc * nc];
        for cz in 0..nc {
            for cx in 0..nc {
                let mut sum = [0u32; 30];
                for kz in cz.saturating_sub(3)..(cz + 4).min(nc) {
                    for kx in cx.saturating_sub(3)..(cx + 4).min(nc) {
                        for b in 0..30 {
                            sum[b] += hist[kz * nc + kx][b] as u32;
                        }
                    }
                }
                // smooth over neighbouring bins (wrapping at 90°) and take the peak
                let (mut best, mut bi) = (0u32, usize::MAX);
                for b in 0..30 {
                    let v = sum[(b + 29) % 30] + 2 * sum[b] + sum[(b + 1) % 30];
                    if v > best {
                        best = v;
                        bi = b;
                    }
                }
                if best >= 40 {
                    cell_dir[cz * nc + cx] = (bi * 3 + 1) as u8 + 1;
                }
            }
        }
        out.rowdir = (0..t)
            .flat_map(|j| {
                let (lc, patterned, parcel_dir, cell_dir) = (&lc, &patterned, &parcel_dir, &cell_dir);
                (0..t).map(move |i| {
                    let p = (j + h) * n + i + h;
                    if !patterned[lc[p] as usize] {
                        0
                    } else if parcel_dir[p] != 0 {
                        parcel_dir[p]
                    } else {
                        cell_dir[(p / n / CS) * nc + (p % n) / CS]
                    }
                })
            })
            .collect();
    }
    // ---- measured trees (Shizuoka)
    // LiDAR tree tops are not taken on buildings, roads, railways or bridge decks
    let mut busy: Vec<bool> = (0..t * t).map(|p| out.bldg[p] != 0 || out.road[p] != 0 || out.rail[p] != 0).collect();
    for f in structs.iter().filter(|f| f.flags & (crate::osm::F_BRIDGE | crate::osm::F_EMBANKMENT) != 0) {
        let Some(pp) = f.parts.first() else { continue };
        let px: Vec<[f64; 2]> = pp.iter().map(|q| [q[0] - mx0 as f64, q[1] - mz0 as f64]).collect();
        for k in 0..px.len().saturating_sub(1) {
            let (a, b) = (px[k], px[k + 1]);
            let steps = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs()).ceil() as usize).max(1);
            for st in 0..=steps {
                let u = st as f64 / steps as f64;
                let (cx, cz) = (a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u);
                for dz in -6i64..=6 {
                    for dx in -6i64..=6 {
                        let (x, z) = (cx.floor() as i64 + dx, cz.floor() as i64 + dz);
                        if x >= 0 && z >= 0 && x < TILE && z < TILE {
                            busy[z as usize * t + x as usize] = true;
                        }
                    }
                }
            }
        }
    }
    let (trees, tcov, nt) = tree_layer(ctx, tx, tz, &busy);
    out.trees = trees;
    out.tcov = tcov;
    out.ntrees = nt;
    // the measured canopy surface, so dense crowns close up as in the photo
    out.canopy = vec![0u8; t * t];
    if let Some((chm, _)) = &chm_core {
        for p in 0..t * t {
            if chm[p] >= 20 && !busy[p] {
                out.canopy[p] = chm[p];
            }
        }
    }
    Ok(out)
}

/// Direction (degrees, 0..180) of the long side of a ring's minimum-area
/// bounding rectangle.
fn long_axis_deg(ring: &[[f64; 2]]) -> Option<f64> {
    let mut pts: Vec<[f64; 2]> = ring.to_vec();
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    pts.dedup();
    if pts.len() < 3 {
        return None;
    }
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut hull: Vec<[f64; 2]> = Vec::new();
    for pass in 0..2 {
        let start = hull.len();
        let it: Box<dyn Iterator<Item = &[f64; 2]>> = if pass == 0 { Box::new(pts.iter()) } else { Box::new(pts.iter().rev()) };
        for &q in it {
            while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], q) <= 0.0 {
                hull.pop();
            }
            hull.push(q);
        }
        hull.pop();
    }
    let mut best: Option<(f64, f64)> = None; // (area, long-side angle)
    for k in 0..hull.len() {
        let (a, b) = (hull[k], hull[(k + 1) % hull.len()]);
        let th = (b[1] - a[1]).atan2(b[0] - a[0]);
        let (c, s) = (th.cos(), th.sin());
        let (mut u0, mut u1, mut v0, mut v1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for q in &hull {
            let (u, v) = (q[0] * c + q[1] * s, -q[0] * s + q[1] * c);
            u0 = u0.min(u);
            u1 = u1.max(u);
            v0 = v0.min(v);
            v1 = v1.max(v);
        }
        let area = (u1 - u0) * (v1 - v0);
        let ang = if u1 - u0 >= v1 - v0 { th } else { th + std::f64::consts::FRAC_PI_2 };
        if best.is_none_or(|bb| area < bb.0) {
            best = Some((area, ang.to_degrees().rem_euclid(180.0)));
        }
    }
    best.map(|b| b.1)
}

/// Measured trees of a tile: forest-analysis tree points, and tree tops found
/// in the LiDAR canopy height model (which replaces the points where it exists).
fn tree_layer(ctx: &Ctx, tx: i64, tz: i64, busy: &[bool]) -> (Vec<u8>, Vec<u8>, usize) {
    let t = TILE as usize;
    let mut tcov = vec![0u8; t * t];
    let fp = crate::trees::read_fpts(&ctx.work, tx, tz);
    let (x0, z0) = (tx * TILE, tz * TILE);
    // planted forest measured tree by tree: 10 m cells holding points, grown by one cell
    let nc = t / 10 + 1;
    let mut cell = vec![false; nc * nc];
    for &(x, z, _, _) in &fp {
        let (lx, lz) = ((x - x0) as usize, (z - z0) as usize);
        cell[(lz / 10) * nc + lx / 10] = true;
    }
    let mut grown = cell.clone();
    for cz in 0..nc {
        for cx in 0..nc {
            if !cell[cz * nc + cx] {
                continue;
            }
            for dz in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (a, b) = (cx as i64 + dx, cz as i64 + dz);
                    if a >= 0 && b >= 0 && (a as usize) < nc && (b as usize) < nc {
                        grown[b as usize * nc + a as usize] = true;
                    }
                }
            }
        }
    }
    tcov.par_chunks_mut(t).enumerate().for_each(|(z, row)| {
        for x in 0..t {
            if grown[(z / 10) * nc + x / 10] {
                row[x] = 1;
            }
        }
    });
    let mut out: Vec<(u16, u16, u16, u8, u8)> = Vec::new();
    let chm = crate::trees::read_chm(&ctx.work, tx, tz);
    if let Some((chm, cov)) = &chm {
        for p in 0..t * t {
            if cov[p] != 0 {
                tcov[p] |= 2;
            }
        }
        // buildings, roads, rails (grown by 2 px) are not trees
        let mut bm: Vec<bool> = busy.to_vec();
        for _ in 0..2 {
            let prev = bm.clone();
            bm.par_chunks_mut(t).enumerate().for_each(|(z, row)| {
                for x in 0..t {
                    if prev[z * t + x] {
                        continue;
                    }
                    row[x] = (x > 0 && prev[z * t + x - 1]) || (x + 1 < t && prev[z * t + x + 1]) || (z > 0 && prev[(z - 1) * t + x]) || (z + 1 < t && prev[(z + 1) * t + x]);
                }
            });
        }
        // 3×3 mean canopy height (m)
        let mut sm = vec![0f32; t * t];
        sm.par_chunks_mut(t).enumerate().for_each(|(z, row)| {
            for x in 0..t {
                let (mut s, mut c) = (0u32, 0u32);
                for dz in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let (a, b) = (x as i64 + dx, z as i64 + dz);
                        if a >= 0 && b >= 0 && (a as usize) < t && (b as usize) < t {
                            s += chm[b as usize * t + a as usize] as u32;
                            c += 1;
                        }
                    }
                }
                row[x] = s as f32 / c as f32 / 5.0;
            }
        });
        // forest points by 4 m cell, for the species of LiDAR trees
        let mut sp_cell: HashMap<(i64, i64), Vec<(i64, i64, u8)>> = HashMap::new();
        for &(x, z, _, s) in &fp {
            sp_cell.entry((x.div_euclid(4), z.div_euclid(4))).or_default().push((x, z, s));
        }
        let tops: Vec<Vec<(u16, u16, u16, u8, u8)>> = (0..t)
            .into_par_iter()
            .map(|z| {
                let mut v = Vec::new();
                for x in 0..t {
                    let p = z * t + x;
                    let hv = sm[p];
                    if hv < 2.5 || bm[p] || cov[p] == 0 {
                        continue;
                    }
                    let r = (1.0 + 0.08 * hv).clamp(1.5, 4.0);
                    let ri = r.ceil() as i64;
                    let mut is_max = true;
                    'scan: for dz in -ri..=ri {
                        for dx in -ri..=ri {
                            if (dx == 0 && dz == 0) || (dx * dx + dz * dz) as f32 > r * r {
                                continue;
                            }
                            let (a, b) = (x as i64 + dx, z as i64 + dz);
                            if a < 0 || b < 0 || a as usize >= t || b as usize >= t {
                                continue;
                            }
                            let q = b as usize * t + a as usize;
                            if sm[q] > hv || (sm[q] == hv && q < p) {
                                is_max = false;
                                break 'scan;
                            }
                        }
                    }
                    if !is_max {
                        continue;
                    }
                    let mut top = 0u8;
                    for dz in -1i64..=1 {
                        for dx in -1i64..=1 {
                            let (a, b) = (x as i64 + dx, z as i64 + dz);
                            if a >= 0 && b >= 0 && (a as usize) < t && (b as usize) < t {
                                top = top.max(chm[b as usize * t + a as usize]);
                            }
                        }
                    }
                    let (gx, gz) = (x0 + x as i64, z0 + z as i64);
                    let mut sp = 0u8;
                    let mut best = 9.0f64;
                    for cz in gz.div_euclid(4) - 1..=gz.div_euclid(4) + 1 {
                        for cx in gx.div_euclid(4) - 1..=gx.div_euclid(4) + 1 {
                            for &(fx, fz, s) in sp_cell.get(&(cx, cz)).map(|v| v.as_slice()).unwrap_or(&[]) {
                                let d = ((fx - gx).pow(2) + (fz - gz).pow(2)) as f64;
                                if d < best {
                                    best = d;
                                    sp = s;
                                }
                            }
                        }
                    }
                    v.push((x as u16, z as u16, (top as u16) * 2, sp, 2));
                }
                v
            })
            .collect();
        out.extend(tops.into_iter().flatten());
    }
    for &(x, z, hh, s) in &fp {
        let (lx, lz) = ((x - x0) as usize, (z - z0) as usize);
        if chm.as_ref().is_some_and(|(_, cov)| cov[lz * t + lx] != 0) {
            continue;
        }
        out.push((lx as u16, lz as u16, (hh * 10.0).round() as u16, s, 1));
    }
    out.sort_unstable();
    let mut b = Vec::with_capacity(out.len() * 8);
    for &(x, z, hh, s, src) in &out {
        b.extend_from_slice(&x.to_le_bytes());
        b.extend_from_slice(&z.to_le_bytes());
        b.extend_from_slice(&hh.to_le_bytes());
        b.push(s);
        b.push(src);
    }
    (b, tcov, out.len())
}

/// Rasterize one 3D roof polygon (pixel coords + height) into `roof` for the
/// pixels of building `id` (max height wins).
fn raster_roof(pts: &[(f64, f64, f32)], n: usize, bldg: &[u32], id: u32, roof: &mut [f32]) {
    if pts.len() < 3 {
        return;
    }
    // plane through the polygon (Newell's method) for heights inside it
    let (mut nx, mut ny, mut nz) = (0.0f64, 0.0f64, 0.0f64);
    let (mut cx, mut cy, mut cz) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..pts.len() {
        let (x0, y0, z0) = (pts[i].0, pts[i].1, pts[i].2 as f64);
        let (x1, y1, z1) = (pts[(i + 1) % pts.len()].0, pts[(i + 1) % pts.len()].1, pts[(i + 1) % pts.len()].2 as f64);
        nx += (y0 - y1) * (z0 + z1);
        ny += (z0 - z1) * (x0 + x1);
        nz += (x0 - x1) * (y0 + y1);
        cx += x0;
        cy += y0;
        cz += z0;
    }
    let m = pts.len() as f64;
    let (cx, cy, cz) = (cx / m, cy / m, cz / m);
    if nz.abs() < 1e-9 {
        return; // vertical face
    }
    let ring: Vec<(f64, f64)> = pts.iter().map(|p| (p.0, p.1)).collect();
    raster::fill_polygon(&[ring], n, n, |r, c0, c1| {
        for c in c0..c1 {
            let p = r * n + c;
            if bldg[p] != id {
                continue;
            }
            let (x, y) = (c as f64 + 0.5, r as f64 + 0.5);
            let z = (cz - (nx * (x - cx) + ny * (y - cy)) / nz) as f32;
            if roof[p].is_nan() || z > roof[p] {
                roof[p] = z;
            }
        }
    });
}

/// Write the layers of one tile (only files whose content changed).
pub fn write_tile(ctx: &Ctx, tx: i64, tz: i64, o: &TileOut) -> Result<bool> {
    let (x, z) = (tx * TILE, tz * TILE);
    let t = TILE as usize;
    let mut changed = false;
    let p = |l: &str, e: &str| ctx.out.join(layer_name(l, x, z, e));
    let u16b: &[u8] = unsafe { std::slice::from_raw_parts(o.lc.as_ptr() as *const u8, o.lc.len() * 2) };
    let u32b: &[u8] = unsafe { std::slice::from_raw_parts(o.bldg.as_ptr() as *const u8, o.bldg.len() * 4) };
    changed |= tiff::write(&p("lc", "tif"), Fmt::U16, t, t, u16b, x, z, Some("0"), 6)?;
    changed |= tiff::write(&p("bldg", "tif"), Fmt::U32, t, t, u32b, x, z, Some("0"), 6)?;
    changed |= tiff::write(&p("roof", "tif"), Fmt::F32, t, t, tiff::f32_bytes(&o.roof), x, z, None, 6)?;
    changed |= tiff::write(&p("struct", "tif"), Fmt::U8, t, t, &o.strct, x, z, Some("0"), 6)?;
    changed |= tiff::write(&p("road", "tif"), Fmt::U8, t, t, &o.road, x, z, Some("0"), 6)?;
    changed |= tiff::write(&p("roadh", "tif"), Fmt::F32, t, t, tiff::f32_bytes(&o.roadh), x, z, None, 6)?;
    changed |= tiff::write(&p("rail", "tif"), Fmt::U8, t, t, &o.rail, x, z, Some("0"), 6)?;
    changed |= tiff::write(&p("railh", "tif"), Fmt::F32, t, t, tiff::f32_bytes(&o.railh), x, z, None, 6)?;
    changed |= tiff::write(&p("tcov", "tif"), Fmt::U8, t, t, &o.tcov, x, z, Some("0"), 6)?;
    changed |= tiff::write(&p("lake", "tif"), Fmt::F32, t, t, tiff::f32_bytes(&o.lake), x, z, None, 6)?;
    changed |= tiff::write(&p("rowdir", "tif"), Fmt::U8, t, t, &o.rowdir, x, z, Some("0"), 6)?;
    changed |= tiff::write(&p("canopy", "tif"), Fmt::U8, t, t, &o.canopy, x, z, Some("0"), 6)?;
    let tp = p("trees", "bin");
    if std::fs::read(&tp).map(|d| d != o.trees).unwrap_or(true) {
        std::fs::write(&tp, &o.trees)?;
        changed = true;
    }
    let mut tb = Vec::with_capacity(o.table.len() * BldRec::SIZE);
    for r in &o.table {
        r.write(&mut tb);
    }
    let bp = p("bldg", "bin");
    if std::fs::read(&bp).map(|d| d != tb).unwrap_or(true) {
        std::fs::write(&bp, &tb)?;
        changed = true;
    }
    Ok(changed)
}

pub fn run(ctx: &Ctx, tiles: &[(i64, i64)], concurrency: usize) -> Result<()> {
    std::fs::create_dir_all(&ctx.out)?;
    let t0 = std::time::Instant::now();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let changed = std::sync::atomic::AtomicUsize::new(0);
    for chunk in tiles.chunks(concurrency.max(1)) {
        chunk.par_iter().try_for_each(|&(tx, tz)| -> Result<()> {
            let t = std::time::Instant::now();
            let o = process_tile(ctx, tx, tz)?;
            let c = write_tile(ctx, tx, tz, &o)?;
            if c {
                changed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            eprintln!(
                "  features X={} Z={}: buildings PLATEAU {} 基盤 {}, roads {}, rails {}, measured trees {} {:.1}s{} [{d}/{} {:.0}s]",
                tx * TILE,
                tz * TILE,
                o.stats[0],
                o.stats[1],
                o.stats[2],
                o.stats[3],
                o.ntrees,
                t.elapsed().as_secs_f64(),
                if c { "" } else { " (unchanged)" },
                tiles.len(),
                t0.elapsed().as_secs_f64()
            );
            Ok(())
        })?;
    }
    eprintln!("features: {} tiles, {} changed, {:.0}s", tiles.len(), changed.into_inner(), t0.elapsed().as_secs_f64());
    let mut f = std::io::BufWriter::new(std::fs::File::create(ctx.out.join("landcover_classes.csv"))?);
    writeln!(f, "index,{}", crate::landcover::HEADER)?;
    for (i, c) in ctx.classes.iter().enumerate() {
        writeln!(f, "{},{},{}", i + 1, c.key, c.name)?;
    }
    Ok(())
}

/// Chamfer distance (pixels, diagonal √2) to the nearest finite value of
/// `src` (capped), and that value.
fn nearest_value(src: &[f32], w: usize, h: usize, cap: f32) -> (Vec<f32>, Vec<f32>) {
    let mut d: Vec<f32> = src.iter().map(|v| if v.is_finite() { 0.0 } else { cap }).collect();
    let mut val = src.to_vec();
    const DG: f32 = std::f32::consts::SQRT_2;
    for pass in 0..2 {
        let (ys, xs): (Vec<usize>, Vec<usize>) = if pass == 0 { ((0..h).collect(), (0..w).collect()) } else { ((0..h).rev().collect(), (0..w).rev().collect()) };
        let s: i64 = if pass == 0 { -1 } else { 1 };
        for &y in &ys {
            for &x in &xs {
                let i = y * w + x;
                let (mut best, mut bv) = (d[i], val[i]);
                for (dx, dy, c) in [(s, 0i64, 1.0f32), (0, s, 1.0), (s, s, DG), (-s, s, DG)] {
                    let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                    if xx < 0 || yy < 0 || xx >= w as i64 || yy >= h as i64 {
                        continue;
                    }
                    let j = yy as usize * w + xx as usize;
                    if d[j] + c < best {
                        best = d[j] + c;
                        bv = val[j];
                    }
                }
                d[i] = best;
                val[i] = bv;
            }
        }
    }
    (d, val)
}
