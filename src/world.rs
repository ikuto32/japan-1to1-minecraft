//! Minecraft Java Edition 1.21.11 world writer (Anvil regions) from the
//! heightmap tiles. World height Y = -2032..2031, elevation 0 m = Y -1760.

use crate::heightmap::{self, K_HOLE, K_INLAND, K_LAND1, K_LAND5, K_OUT, K_SEA, TILE};
use crate::nbt::{self, Tag, W};
use crate::{region, tiff};
use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

pub const DATA_VERSION: i32 = 4671;
pub const MIN_Y: i32 = -2032;
pub const HEIGHT: i32 = 4064;
pub const SEA_Y: i32 = -1760; // elevation 0 m
pub const SEC_MIN: i32 = MIN_Y / 16; // -127
pub const N_SEC: i32 = HEIGHT / 16; // 254
const HALO: usize = 32;
const RW: usize = 512 + 2 * HALO; // region window width

#[inline]
fn hash2(x: i64, z: i64, salt: u64) -> u32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (z as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ salt;
    h ^= h >> 31;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 29;
    (h >> 32) as u32
}

/// Smooth value noise in [0,1) with the given cell size (for patchy materials).
pub fn vnoise(x: i64, z: i64, cell: i64, salt: u64) -> f32 {
    let (cx, cz) = (x.div_euclid(cell), z.div_euclid(cell));
    let (fx, fz) = ((x.rem_euclid(cell)) as f32 / cell as f32, (z.rem_euclid(cell)) as f32 / cell as f32);
    let v = |a: i64, b: i64| hash2(a, b, salt) as f32 / u32::MAX as f32;
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sz) = (s(fx), s(fz));
    let t = v(cx, cz) * (1.0 - sx) + v(cx + 1, cz) * sx;
    let b = v(cx, cz + 1) * (1.0 - sx) + v(cx + 1, cz + 1) * sx;
    t * (1.0 - sz) + b * sz
}

/// Heights/classes of a region plus a HALO-wide border.
pub struct RegionInput {
    pub ox: i64, // MC X of window[0]
    pub oz: i64,
    pub h: Vec<f32>,
    pub k: Vec<u8>,
}

// ---------------------------------------------------------------- inputs

type Shared<T> = std::sync::Arc<std::sync::OnceLock<Option<std::sync::Arc<T>>>>;

/// Decoded 512×512 blocks of the heightmap tiles, read on demand. Each tile
/// file is opened and each block decoded once, however many threads ask for
/// it at the same time (the others wait on the OnceLock).
pub struct TileSet {
    dir: PathBuf,
    blocks: std::sync::Mutex<HashMap<(i64, i64), Shared<(Vec<f32>, Vec<u8>)>>>,
    readers: std::sync::Mutex<HashMap<(i64, i64), Shared<(tiff::Reader, tiff::Reader)>>>,
}

impl TileSet {
    pub fn new(dir: &Path) -> Self {
        TileSet { dir: dir.to_path_buf(), blocks: std::sync::Mutex::new(HashMap::new()), readers: std::sync::Mutex::new(HashMap::new()) }
    }

    fn reader(&self, tx: i64, tz: i64) -> Result<Option<std::sync::Arc<(tiff::Reader, tiff::Reader)>>> {
        let cell = {
            let mut m = self.readers.lock().unwrap();
            // tiles are processed in raster order; a tile needs itself and its 8 neighbours
            if m.len() > 24 && !m.contains_key(&(tx, tz)) {
                m.clear();
            }
            m.entry((tx, tz)).or_default().clone()
        };
        let mut err = None;
        let r = cell.get_or_init(|| {
            let hp = self.dir.join(heightmap::height_name(tx * TILE, tz * TILE));
            let kp = self.dir.join(heightmap::class_name(tx * TILE, tz * TILE));
            if !(hp.exists() && kp.exists()) {
                return None;
            }
            match (tiff::Reader::open(&hp), tiff::Reader::open(&kp)) {
                (Ok(a), Ok(b)) => Some(std::sync::Arc::new((a, b))),
                (Err(e), _) | (_, Err(e)) => {
                    err = Some(e);
                    None
                }
            }
        });
        if let Some(e) = err {
            return Err(e);
        }
        Ok(r.clone())
    }

    /// Cached decoded block (each block is needed by up to 9 regions).
    fn block_cached(&self, bx: i64, bz: i64) -> Result<Option<std::sync::Arc<(Vec<f32>, Vec<u8>)>>> {
        let cell = {
            let mut m = self.blocks.lock().unwrap();
            if m.len() > 2000 && !m.contains_key(&(bx, bz)) {
                m.clear();
            }
            m.entry((bx, bz)).or_default().clone()
        };
        let mut err = None;
        let b = cell.get_or_init(|| match self.block(bx, bz) {
            Ok(v) => v.map(std::sync::Arc::new),
            Err(e) => {
                err = Some(e);
                None
            }
        });
        if let Some(e) = err {
            return Err(e);
        }
        Ok(b.clone())
    }

    /// Classes of an arbitrary window (missing data = outside).
    pub fn classes(&self, x0: i64, z0: i64, w: usize, h: usize) -> Result<Vec<u8>> {
        let mut kv = vec![K_OUT; w * h];
        for bz in z0.div_euclid(512)..=(z0 + h as i64 - 1).div_euclid(512) {
            for bx in x0.div_euclid(512)..=(x0 + w as i64 - 1).div_euclid(512) {
                let Some(b) = self.block_cached(bx, bz)? else { continue };
                let (zs, ze) = ((bz * 512).max(z0), ((bz + 1) * 512).min(z0 + h as i64));
                let (xs, xe) = ((bx * 512).max(x0), ((bx + 1) * 512).min(x0 + w as i64));
                for z in zs..ze {
                    let src = ((z - bz * 512) * 512 + (xs - bx * 512)) as usize;
                    let dst = ((z - z0) as usize) * w + (xs - x0) as usize;
                    let len = (xe - xs) as usize;
                    kv[dst..dst + len].copy_from_slice(&b.1[src..src + len]);
                }
            }
        }
        Ok(kv)
    }

    /// Heights/classes of an arbitrary window (missing data = outside).
    pub fn window(&self, x0: i64, z0: i64, w: usize, h: usize) -> Result<(Vec<f32>, Vec<u8>)> {
        let mut hv = vec![0f32; w * h];
        let mut kv = vec![K_OUT; w * h];
        for bz in z0.div_euclid(512)..=(z0 + h as i64 - 1).div_euclid(512) {
            for bx in x0.div_euclid(512)..=(x0 + w as i64 - 1).div_euclid(512) {
                let Some(b) = self.block_cached(bx, bz)? else { continue };
                let (zs, ze) = ((bz * 512).max(z0), ((bz + 1) * 512).min(z0 + h as i64));
                let (xs, xe) = ((bx * 512).max(x0), ((bx + 1) * 512).min(x0 + w as i64));
                for z in zs..ze {
                    let src = ((z - bz * 512) * 512 + (xs - bx * 512)) as usize;
                    let dst = ((z - z0) as usize) * w + (xs - x0) as usize;
                    let len = (xe - xs) as usize;
                    hv[dst..dst + len].copy_from_slice(&b.0[src..src + len]);
                    kv[dst..dst + len].copy_from_slice(&b.1[src..src + len]);
                }
            }
        }
        Ok((hv, kv))
    }

    /// Heightmap value and class at a Minecraft column (None outside the data).
    pub fn sample(&self, x: i64, z: i64) -> Result<Option<(f32, u8)>> {
        let Some(b) = self.block_cached(x.div_euclid(512), z.div_euclid(512))? else { return Ok(None) };
        let i = (z.rem_euclid(512) * 512 + x.rem_euclid(512)) as usize;
        if b.1[i] == K_OUT {
            return Ok(None);
        }
        Ok(Some((b.0[i], b.1[i])))
    }

    /// Mean heightmap value over the (2r+1)² columns around (x, z) (None if none inside).
    pub fn mean_height(&self, x: f64, z: f64, r: i64) -> Result<Option<f32>> {
        let (cx, cz) = (x.floor() as i64, z.floor() as i64);
        let (mut s, mut n) = (0f64, 0u32);
        for dz in -r..=r {
            for dx in -r..=r {
                if let Some((h, _)) = self.sample(cx + dx, cz + dz)? {
                    s += h as f64;
                    n += 1;
                }
            }
        }
        Ok((n > 0).then(|| (s / n as f64) as f32))
    }

    /// Heights/classes of one 512 block at block coords (bx, bz) = MC / 512.
    fn block(&self, bx: i64, bz: i64) -> Result<Option<(Vec<f32>, Vec<u8>)>> {
        let per = TILE / 512;
        let (tx, tz) = (bx.div_euclid(per), bz.div_euclid(per));
        let Some(r) = self.reader(tx, tz)? else { return Ok(None) };
        let (ix, iz) = (bx.rem_euclid(per) as usize, bz.rem_euclid(per) as usize);
        Ok(Some((tiff::bytes_to_f32(r.0.tile(ix, iz)?), r.1.tile(ix, iz)?)))
    }

    /// Region input with HALO border (missing data = outside).
    pub fn region_input(&self, rx: i64, rz: i64) -> Result<Option<RegionInput>> {
        let Some(center) = self.block_cached(rx, rz)? else { return Ok(None) };
        if center.1.iter().all(|&c| c == K_OUT) {
            return Ok(None);
        }
        let ox = rx * 512 - HALO as i64;
        let oz = rz * 512 - HALO as i64;
        let mut h = vec![0f32; RW * RW];
        let mut k = vec![K_OUT; RW * RW];
        for dz in -1i64..=1 {
            for dx in -1i64..=1 {
                let blk = if dx == 0 && dz == 0 { Some(center.clone()) } else { self.block_cached(rx + dx, rz + dz)? };
                let Some(blk) = blk else { continue };
                let (bh, bk) = (&blk.0, &blk.1);
                // copy the part of this block that falls into the window
                let bx0 = (rx + dx) * 512;
                let bz0 = (rz + dz) * 512;
                for y in 0..512i64 {
                    let wy = bz0 + y - oz;
                    if wy < 0 || wy >= RW as i64 {
                        continue;
                    }
                    for x in 0..512i64 {
                        let wx = bx0 + x - ox;
                        if wx < 0 || wx >= RW as i64 {
                            continue;
                        }
                        let (wi, bi) = (wy as usize * RW + wx as usize, (y * 512 + x) as usize);
                        h[wi] = bh[bi];
                        k[wi] = bk[bi];
                    }
                }
            }
        }
        Ok(Some(RegionInput { ox, oz, h, k }))
    }
}

// ---------------------------------------------------------------- level.dat etc.

const LEVEL_TEMPLATE: &[u8] = include_bytes!("../assets/level_template.dat");

pub fn write_level(world: &Path, name: &str, spawn: (i64, i64, i64)) -> Result<()> {
    use std::io::Read;
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(LEVEL_TEMPLATE).read_to_end(&mut raw)?;
    let (_, mut root) = nbt::read_root(&raw)?;
    let data = root.get_mut("Data").context("Data")?;
    data.set("LevelName", Tag::String(name.into()));
    data.set("GameType", Tag::Int(1)); // creative
    data.set("allowCommands", Tag::Byte(1));
    data.set("Difficulty", Tag::Byte(2));
    data.set("DayTime", Tag::Long(6000));
    data.set("Time", Tag::Long(0));
    data.set("raining", Tag::Byte(0));
    data.set("thundering", Tag::Byte(0));
    data.set("initialized", Tag::Byte(1));
    data.set(
        "LastPlayed",
        Tag::Long(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)),
    );
    data.set(
        "spawn",
        Tag::Compound(vec![
            ("pos".into(), Tag::IntArray(vec![spawn.0 as i32, spawn.1 as i32, spawn.2 as i32])),
            ("dimension".into(), Tag::String("minecraft:overworld".into())),
            ("yaw".into(), Tag::Float(0.0)),
            ("pitch".into(), Tag::Float(0.0)),
        ]),
    );
    if let Some(gr) = data.get_mut("game_rules") {
        // spawn exactly at the summit instead of a random point within 10 blocks
        gr.set("minecraft:respawn_radius", Tag::Int(0));
    }
    // void outside the generated area: empty flat world, no features/structures
    let wgs = data.get_mut("WorldGenSettings").context("WorldGenSettings")?;
    wgs.set("generate_features", Tag::Byte(0));
    wgs.set("bonus_chest", Tag::Byte(0));
    let ow = wgs.get_mut("dimensions").and_then(|d| d.get_mut("minecraft:overworld")).context("overworld")?;
    ow.set("type", Tag::String("minecraft:overworld".into()));
    ow.set(
        "generator",
        Tag::Compound(vec![
            ("type".into(), Tag::String("minecraft:flat".into())),
            (
                "settings".into(),
                Tag::Compound(vec![
                    ("biome".into(), Tag::String("minecraft:the_void".into())),
                    ("features".into(), Tag::Byte(0)),
                    ("lakes".into(), Tag::Byte(0)),
                    ("layers".into(), Tag::List(vec![])),
                    ("structure_overrides".into(), Tag::List(vec![])),
                ]),
            ),
        ]),
    );
    data.set(
        "DataPacks",
        Tag::Compound(vec![
            ("Enabled".into(), Tag::List(vec![Tag::String("vanilla".into()), Tag::String("file/tall_japan".into())])),
            (
                "Disabled".into(),
                Tag::List(vec![
                    Tag::String("minecart_improvements".into()),
                    Tag::String("redstone_experiments".into()),
                    Tag::String("trade_rebalance".into()),
                ]),
            ),
        ]),
    );
    let bytes = nbt::write_root("", &root);
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(&bytes)?;
    let gz = e.finish()?;
    std::fs::write(world.join("level.dat"), gz)?;
    Ok(())
}

const OVERWORLD_TYPE: &str = include_str!("../assets/overworld_dimension_type.json");

/// Copy the in-game tools data pack (assets/datapacks/japan_tools, made by
/// tools/gen_datapack.py) into the world, replacing an older copy.
pub fn install_tools_pack(world: &Path) -> Result<()> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("datapacks").join("japan_tools");
    if !src.join("pack.mcmeta").exists() {
        return Ok(());
    }
    let dst = world.join("datapacks").join("japan_tools");
    if dst.exists() {
        std::fs::remove_dir_all(&dst)?;
    }
    let mut stack = vec![src.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let p = e?.path();
            let rel = p.strip_prefix(&src)?;
            if p.is_dir() {
                stack.push(p.clone());
                std::fs::create_dir_all(dst.join(rel))?;
            } else {
                std::fs::create_dir_all(dst.join(rel).parent().unwrap())?;
                std::fs::copy(&p, dst.join(rel))?;
            }
        }
    }
    Ok(())
}

/// The non-region part of a world (fresh level.dat, data packs, icon,
/// attribution) — the "base" package of a release.
pub fn write_base(hm_dir: &Path, world: &Path, name: &str) -> Result<()> {
    std::fs::create_dir_all(world)?;
    let ts = TileSet::new(hm_dir);
    let peak = find_peak(&ts, 20600, 70700, 600)?;
    let spawn = (peak.0, SEA_Y as i64 + peak.1.round() as i64 + 1, peak.2);
    write_level(world, name, spawn)?;
    write_icon(&ts, world, spawn.0, spawn.2)?;
    write_datapack(world)?;
    install_tools_pack(world)?;
    std::fs::write(world.join("ATTRIBUTION.md"), include_str!("../assets/ATTRIBUTION.md"))?;
    Ok(())
}

pub fn write_datapack(world: &Path) -> Result<()> {
    let dp = world.join("datapacks").join("tall_japan");
    let dt = dp.join("data").join("minecraft").join("dimension_type");
    std::fs::create_dir_all(&dt)?;
    std::fs::write(
        dp.join("pack.mcmeta"),
        "{\n  \"pack\": {\n    \"description\": \"Japan DEM world: overworld height Y -2032..2031 (elevation 0 m = Y -1760)\",\n    \"min_format\": [94, 1],\n    \"max_format\": [94, 1]\n  }\n}\n",
    )?;
    std::fs::write(dt.join("overworld.json"), OVERWORLD_TYPE)?;
    Ok(())
}

/// Highest heightmap pixel within `r` blocks of (x, z): (X, elevation, Z).
pub fn find_peak(ts: &TileSet, x: i64, z: i64, r: i64) -> Result<(i64, f32, i64)> {
    let mut best = (x, f32::MIN, z);
    for bz in (z - r).div_euclid(512)..=(z + r).div_euclid(512) {
        for bx in (x - r).div_euclid(512)..=(x + r).div_euclid(512) {
            let Some((h, k)) = ts.block(bx, bz)? else { continue };
            for j in 0..512i64 {
                for i in 0..512i64 {
                    let (wx, wz) = (bx * 512 + i, bz * 512 + j);
                    if (wx - x).pow(2) + (wz - z).pow(2) > r * r {
                        continue;
                    }
                    let idx = (j * 512 + i) as usize;
                    if k[idx] != K_OUT && h[idx] > best.1 {
                        best = (wx, h[idx], wz);
                    }
                }
            }
        }
    }
    if best.1 == f32::MIN {
        bail!("no heightmap data around spawn");
    }
    Ok(best)
}

pub static T_INPUT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static T_COLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static T_LIGHT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static T_NBT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static T_ZLIB: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static T_WRITE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static T_FEAT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static T_STRUCT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static T_BLDG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static T_DECOR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn tadd(a: &std::sync::atomic::AtomicU64, t: std::time::Instant) {
    a.fetch_add(t.elapsed().as_micros() as u64, std::sync::atomic::Ordering::Relaxed);
}

/// Bump when the generator writes something else from the same inputs: the
/// next `world` run then rebuilds every region once.
pub const GEN_VERSION: u64 = 3;

/// Fast 64-bit hash for change detection (not for security).
struct Hasher(u64);

impl Hasher {
    fn new(seed: u64) -> Self {
        Hasher(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    #[inline]
    fn word(&mut self, w: u64) {
        self.0 = (self.0 ^ w).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(29);
    }
    fn bytes(&mut self, b: &[u8]) {
        let mut it = b.chunks_exact(8);
        for c in &mut it {
            self.word(u64::from_le_bytes(c.try_into().unwrap()));
        }
        let mut last = [0u8; 8];
        last[..it.remainder().len()].copy_from_slice(it.remainder());
        self.word(u64::from_le_bytes(last));
        self.word(b.len() as u64);
    }
    fn slice<T: Copy>(&mut self, v: &[T]) {
        // SAFETY: plain numbers (u8/u16/u32/f32) read as their bytes
        self.bytes(unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) });
    }
    fn finish(&self) -> u64 {
        let mut x = self.0;
        x ^= x >> 33;
        x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        x ^ (x >> 33)
    }
}

/// Everything a region is built from: the heightmap and feature windows,
/// the building records and trees in them, the bridge/tunnel profiles
/// around, and (in `global`) the mapping table and the generator version.
fn region_digest(global: u64, inp: &RegionInput, fw: &crate::rgen::FeatWin, structs: &[crate::vfeat::VFeat]) -> u64 {
    let mut h = Hasher::new(global);
    h.word(inp.ox as u64);
    h.word(inp.oz as u64);
    h.slice(&inp.h);
    h.slice(&inp.k);
    h.slice(&fw.lc);
    h.slice(&fw.bldg);
    h.slice(&fw.roof);
    h.slice(&fw.strct);
    h.slice(&fw.road);
    h.slice(&fw.roadh);
    h.slice(&fw.rail);
    h.slice(&fw.railh);
    h.slice(&fw.tcov);
    h.slice(&fw.lake);
    h.slice(&fw.rowdir);
    h.slice(&fw.canopy);
    for t in &fw.trees {
        h.word(t.0 as u64);
        h.word(t.1 as u64);
        h.word(t.2.to_bits() as u64 | (t.3 as u64) << 32);
    }
    let mut seen = vec![false; fw.bld.len()];
    for &k in &fw.bldg {
        if k != 0 && !seen[k as usize] {
            seen[k as usize] = true;
            let r = &fw.bld[k as usize];
            h.word(k as u64);
            h.word(r.src as u64 | (r.usage as u64) << 8 | (r.roof as u64) << 16 | (r.ridge_x as u64) << 24 | (r.storeys as u64) << 32);
            h.word(r.base.to_bits() as u64 | (r.height.to_bits() as u64) << 32);
            h.word(r.seed as u64);
        }
    }
    for f in structs {
        h.word(f.id);
        h.word(f.class as u64 | (f.flags as u64) << 16 | (f.area as u64) << 32);
        h.slice(&f.nums);
        h.bytes(f.text.as_bytes());
        for p in &f.parts {
            h.slice(p);
        }
        for z in &f.zs {
            h.slice(z);
        }
    }
    h.finish()
}

/// The digests the regions of a world were last built from (work folder).
struct Digests {
    path: PathBuf,
    map: std::sync::Mutex<HashMap<(i64, i64), u64>>,
    /// written by another generator version: every region is checked
    stale: bool,
}

impl Digests {
    fn load(path: PathBuf) -> Digests {
        let mut map = HashMap::new();
        let mut stale = true;
        if let Ok(d) = std::fs::read(&path) {
            if d.len() >= 8 && u64::from_le_bytes(d[..8].try_into().unwrap()) == GEN_VERSION {
                stale = false;
                for r in d[8..].chunks_exact(16) {
                    let rx = i32::from_le_bytes(r[..4].try_into().unwrap()) as i64;
                    let rz = i32::from_le_bytes(r[4..8].try_into().unwrap()) as i64;
                    map.insert((rx, rz), u64::from_le_bytes(r[8..].try_into().unwrap()));
                }
            }
        }
        Digests { path, map: std::sync::Mutex::new(map), stale }
    }
    fn get(&self, k: (i64, i64)) -> Option<u64> {
        self.map.lock().unwrap().get(&k).copied()
    }
    fn set(&self, k: (i64, i64), d: u64) {
        self.map.lock().unwrap().insert(k, d);
    }
    fn save(&self) -> Result<()> {
        let map = self.map.lock().unwrap();
        let mut v: Vec<(&(i64, i64), &u64)> = map.iter().collect();
        v.sort();
        let mut b = Vec::with_capacity(8 + 16 * v.len());
        b.extend_from_slice(&GEN_VERSION.to_le_bytes());
        for ((rx, rz), d) in v {
            b.extend_from_slice(&(*rx as i32).to_le_bytes());
            b.extend_from_slice(&(*rz as i32).to_le_bytes());
            b.extend_from_slice(&d.to_le_bytes());
        }
        let tmp = self.path.with_extension("bin.tmp");
        std::fs::write(&tmp, &b)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// A region is up to date when its file is newer than every heightmap tile
/// it reads (its own tile, plus the neighbouring tile when it lies on a tile edge).
fn region_up_to_date(path: &Path, hm_dir: &Path, fs: &crate::chunk::FeatSet, global: Option<std::time::SystemTime>, rx: i64, rz: i64, cache: &std::sync::Mutex<HashMap<(i64, i64), Option<std::time::SystemTime>>>) -> bool {
    let Ok(rt) = std::fs::metadata(path).and_then(|m| m.modified()) else { return false };
    if global.is_some_and(|g| g > rt) {
        return false;
    }
    let per = TILE / 512;
    let mut tiles = Vec::new();
    for dz in -1i64..=1 {
        for dx in -1i64..=1 {
            let t = ((rx + dx).div_euclid(per), (rz + dz).div_euclid(per));
            if !tiles.contains(&t) {
                tiles.push(t);
            }
        }
    }
    for (tx, tz) in tiles {
        let m = *cache.lock().unwrap().entry((tx, tz)).or_insert_with(|| {
            let a = std::fs::metadata(hm_dir.join(heightmap::height_name(tx * TILE, tz * TILE))).and_then(|m| m.modified()).ok();
            let b = std::fs::metadata(hm_dir.join(heightmap::class_name(tx * TILE, tz * TILE))).and_then(|m| m.modified()).ok();
            a.max(b).max(fs.mtime(tx, tz))
        });
        if m.is_some_and(|m| m > rt) {
            return false;
        }
    }
    true
}

/// 64×64 world icon: shaded relief around (x, z), 125 m per pixel.
fn write_icon(ts: &TileSet, world: &Path, x: i64, z: i64) -> Result<()> {
    const N: i64 = 64;
    const S: i64 = 125;
    let mut h = vec![f32::NAN; (N * N) as usize];
    let mut k = vec![K_OUT; (N * N) as usize];
    for j in 0..N {
        for i in 0..N {
            let (wx, wz) = (x + (i - N / 2) * S, z + (j - N / 2) * S);
            if let Some(b) = ts.block_cached(wx.div_euclid(512), wz.div_euclid(512))? {
                let idx = (wz.rem_euclid(512) * 512 + wx.rem_euclid(512)) as usize;
                h[(j * N + i) as usize] = b.0[idx];
                k[(j * N + i) as usize] = b.1[idx];
            }
        }
    }
    let mut rgb = vec![0u8; (N * N * 3) as usize];
    for p in 0..(N * N) as usize {
        let c = match k[p] {
            K_OUT => [0, 0, 0],
            K_SEA => [30, 60, 140],
            K_INLAND => [40, 120, 220],
            _ => {
                let e = h[p].max(0.0);
                let sh = if p >= N as usize + 1 && !h[p - N as usize - 1].is_nan() { ((e - h[p - N as usize - 1].max(0.0)) * 0.004).clamp(-0.35, 0.35) } else { 0.0 };
                let base = if e > 3000.0 { [235.0, 240.0, 245.0] } else if e > 2400.0 { [130.0, 120.0, 115.0] } else { [70.0 + e * 0.04, 130.0 + e * 0.02, 60.0 + e * 0.03] };
                [(base[0] * (1.0 + sh)).min(255.0) as u8, (base[1] * (1.0 + sh)).min(255.0) as u8, (base[2] * (1.0 + sh)).min(255.0) as u8]
            }
        };
        rgb[p * 3..p * 3 + 3].copy_from_slice(&c);
    }
    crate::raster::write_png(&world.join("icon.png"), N as usize, N as usize, &rgb)
}

pub struct RunOpts {
    pub level: u32,
    pub only_regions: Option<Vec<(i64, i64)>>,
    pub force: bool,
}

/// Write the whole world.
pub fn run(hm_dir: &Path, feat_dir: &Path, work: &Path, world: &Path, name: &str, st: &crate::rgen::SpecTable, global_deps: &[PathBuf], content_deps: &[PathBuf], o: &RunOpts) -> Result<()> {
    let t0 = std::time::Instant::now();
    let fs = crate::chunk::FeatSet::new(feat_dir);
    let structs = crate::vfeat::TiledReader::open(&work.join("vec_net_struct.bin"))?;
    // newest global input (mapping table, bridge/tunnel profiles): regions older than it are rebuilt
    let global_mtime = global_deps.iter().filter_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok()).max();
    // a region whose file is older than its inputs is rebuilt only when the
    // inputs themselves changed since it was built (digests in the work folder)
    let digests = Digests::load(work.join(format!("region_digests_{}.bin", world.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default())));
    let global = {
        let mut h = Hasher::new(GEN_VERSION);
        h.word(DATA_VERSION as u64);
        h.word(o.level as u64);
        for p in content_deps {
            h.bytes(&std::fs::read(p).unwrap_or_default());
        }
        h.finish()
    };
    let reg_dir = world.join("region");
    std::fs::create_dir_all(&reg_dir)?;
    write_datapack(world)?;
    install_tools_pack(world)?;
    // sources, licences and attribution travel with the world
    std::fs::write(world.join("ATTRIBUTION.md"), include_str!("../assets/ATTRIBUTION.md"))?;
    let ts = TileSet::new(hm_dir);
    let peak = find_peak(&ts, 20600, 70700, 600)?;
    let spawn = (peak.0, SEA_Y as i64 + peak.1.round() as i64 + 1, peak.2);
    // level.dat also holds the singleplayer player data: never overwrite it
    if !world.join("level.dat").exists() {
        write_level(world, name, spawn)?;
    }
    if !world.join("icon.png").exists() {
        write_icon(&ts, world, spawn.0, spawn.2)?;
    }
    eprintln!("world: spawn at summit X={} Y={} Z={} (elevation {:.2} m)", spawn.0, spawn.1, spawn.2, peak.1);
    // regions = 512 blocks of every heightmap tile
    let mut regions: Vec<(i64, i64)> = match &o.only_regions {
        Some(v) => v.clone(),
        None => {
            let mut v = Vec::new();
            for e in std::fs::read_dir(hm_dir)? {
                let name = e?.file_name().to_string_lossy().to_string();
                if let Some(rest) = name.strip_prefix("height_X").and_then(|s| s.strip_suffix(".tif")) {
                    let (xs, zs) = rest.split_once("_Z").context("tile name")?;
                    let (x, z): (i64, i64) = (xs.parse()?, zs.parse()?);
                    for j in 0..TILE / 512 {
                        for i in 0..TILE / 512 {
                            v.push((x / 512 + i, z / 512 + j));
                        }
                    }
                }
            }
            v
        }
    };
    // tile by tile (regions of one tile in parallel) so each tile file is
    // read only a few times
    let per = TILE / 512;
    let mut by_tile: std::collections::BTreeMap<(i64, i64), Vec<(i64, i64)>> = std::collections::BTreeMap::new();
    for &(rx, rz) in &regions {
        by_tile.entry((rz.div_euclid(per), rx.div_euclid(per))).or_default().push((rx, rz));
    }
    regions.clear();
    let total: usize = by_tile.values().map(|v| v.len()).sum();
    eprintln!("world: {} candidate regions in {} tiles", total, by_tile.len());
    let mtimes = std::sync::Mutex::new(HashMap::new());
    let done = std::sync::atomic::AtomicUsize::new(0);
    let written = std::sync::atomic::AtomicUsize::new(0);
    let chunks = std::sync::atomic::AtomicUsize::new(0);
    let bytes = std::sync::atomic::AtomicU64::new(0);
    let same = std::sync::atomic::AtomicUsize::new(0);
    for (_, tile_regions) in by_tile {
    tile_regions.par_iter().try_for_each(|&(rx, rz)| -> Result<()> {
        let path = reg_dir.join(format!("r.{rx}.{rz}.mca"));
        if !o.force && !digests.stale && region_up_to_date(&path, hm_dir, &fs, global_mtime, rx, rz, &mtimes) {
            done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok(());
        }
        let t = std::time::Instant::now();
        let inp = ts.region_input(rx, rz)?;
        tadd(&T_INPUT, t);
        if let Some(inp) = inp {
            let (x0, z0, x1, z1) = (inp.ox as f64, inp.oz as f64, (inp.ox + RW as i64) as f64, (inp.oz + RW as i64) as f64);
            let tile_structs: Vec<crate::vfeat::VFeat> = structs
                .tile(rx.div_euclid(per), rz.div_euclid(per))
                .into_iter()
                .filter(|f| {
                    let (a, b, c, d) = f.bbox();
                    c >= x0 - 20.0 && a <= x1 + 20.0 && d >= z0 - 20.0 && b <= z1 + 20.0
                })
                .collect();
            let t = std::time::Instant::now();
            let ex = crate::chunk::Extra { fw: fs.window(inp.ox, inp.oz), st, structs: tile_structs };
            let dg = region_digest(global, &inp, &ex.fw, &ex.structs);
            tadd(&T_FEAT, t);
            if !o.force && digests.get((rx, rz)) == Some(dg) && path.exists() {
                // built from these very inputs: only mark it current
                std::fs::File::options().write(true).open(&path)?.set_modified(std::time::SystemTime::now())?;
                same.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return Ok(());
            }
            digests.set((rx, rz), dg);
            if let Some((data, n)) = crate::chunk::build_region(&inp, &ex, rx, rz, o.level)? {
                let t = std::time::Instant::now();
                let tmp = path.with_extension("mca.tmp");
                std::fs::write(&tmp, &data)?;
                std::fs::rename(&tmp, &path)?;
                tadd(&T_WRITE, t);
                written.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                chunks.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                bytes.fetch_add(data.len() as u64, std::sync::atomic::Ordering::Relaxed);
            }
        }
        let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if d % 500 == 0 || d == total {
            let el = t0.elapsed().as_secs_f64();
            eprintln!(
                "  {d}/{total} regions ({} written, {} chunks, {:.1} GB) {:.0}s elapsed, ~{:.0}s left",
                written.load(std::sync::atomic::Ordering::Relaxed),
                chunks.load(std::sync::atomic::Ordering::Relaxed),
                bytes.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e9,
                el,
                el / d as f64 * (total - d) as f64
            );
        }
        Ok(())
    })?;
    digests.save()?;
    }
    let g = |a: &std::sync::atomic::AtomicU64| a.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6;
    eprintln!(
        "world: thread-seconds input {:.1}, features {:.1}, columns {:.1}, structures {:.1}, buildings {:.1}, patterns+trees {:.1}, sky light {:.1}, chunk nbt {:.1}, zlib {:.1}, write {:.1}",
        g(&T_INPUT),
        g(&T_FEAT),
        g(&T_COLS),
        g(&T_STRUCT),
        g(&T_BLDG),
        g(&T_DECOR),
        g(&T_LIGHT),
        g(&T_NBT),
        g(&T_ZLIB),
        g(&T_WRITE)
    );
    eprintln!(
        "world: done in {:.0}s: {} region files, {} chunks, {:.1} GB ({} older than their inputs but built from the same, kept)",
        t0.elapsed().as_secs_f64(),
        written.into_inner(),
        chunks.into_inner(),
        bytes.into_inner() as f64 / 1e9,
        same.into_inner()
    );
    Ok(())
}
