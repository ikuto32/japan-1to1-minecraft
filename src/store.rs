//! DEM storage on a global lat/lon lattice anchored at 46°N, 122°E.
//!
//! Each level (1m / 5m / 10m) is stored in blocks (3rd-mesh sized for 1m/5m,
//! 2nd-mesh sized for 10m). Cell (row, col) has its centre at
//! lat = 46 - (row + 0.5) * cell, lon = 122 + (col + 0.5) * cell.
//! Caches are flat binary files (memory-mapped) built once from the GML.

use crate::gml::{self, NODATA, is_elev};
use crate::scan::DemEntry;
use anyhow::{Result, anyhow, bail};
use std::collections::HashMap;
use std::path::Path;

pub const ORIGIN_LAT: f64 = 46.0;
pub const ORIGIN_LON: f64 = 122.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lv {
    M1,
    M5,
    M10,
}

impl Lv {
    /// cell size in arc-seconds
    pub fn cell_s(self) -> f64 {
        match self {
            Lv::M1 => 0.04,
            Lv::M5 => 0.2,
            Lv::M10 => 0.4,
        }
    }
    /// block size in cells (rows, cols)
    pub fn block(self) -> (usize, usize) {
        match self {
            Lv::M1 => (750, 1125),
            Lv::M5 => (150, 225),
            Lv::M10 => (750, 1125),
        }
    }
    /// cells per degree
    pub fn per_deg(self) -> f64 {
        3600.0 / self.cell_s()
    }
    pub fn name(self) -> &'static str {
        match self {
            Lv::M1 => "1m",
            Lv::M5 => "5m",
            Lv::M10 => "10m",
        }
    }
}

#[inline]
pub fn bkey(brow: i64, bcol: i64) -> u64 {
    ((brow as u64) << 32) | (bcol as u32 as u64)
}

/// South-west corner (lat, lon) and size (dlat, dlon) in degrees of a mesh code
/// (4 = 1st, 6 = 2nd, 8 = 3rd mesh).
pub fn mesh_bounds(code: &str) -> Option<(f64, f64, f64, f64)> {
    let d: Vec<u32> = code.bytes().map(|b| b.wrapping_sub(b'0') as u32).collect();
    if d.len() < 4 || d.iter().any(|&x| x > 9) {
        return None;
    }
    let p = (d[0] * 10 + d[1]) as f64;
    let u = (d[2] * 10 + d[3]) as f64;
    let mut lat = p / 1.5;
    let mut lon = u + 100.0;
    let (mut dlat, mut dlon) = (2.0 / 3.0, 1.0);
    if d.len() >= 6 {
        dlat /= 8.0;
        dlon /= 8.0;
        lat += d[4] as f64 * dlat;
        lon += d[5] as f64 * dlon;
    }
    if d.len() >= 8 {
        dlat /= 10.0;
        dlon /= 10.0;
        lat += d[6] as f64 * dlat;
        lon += d[7] as f64 * dlon;
    }
    Some((lat, lon, dlat, dlon))
}

/// Block key of the block whose north-west corner is the NW corner of `mesh`.
pub fn mesh_block_key(lv: Lv, mesh: &str) -> Option<u64> {
    let (lat, lon, dlat, _) = mesh_bounds(mesh)?;
    let pd = lv.per_deg();
    let (bh, bw) = lv.block();
    let r = ((ORIGIN_LAT - (lat + dlat)) * pd).round() as i64;
    let c = ((lon - ORIGIN_LON) * pd).round() as i64;
    Some(bkey(r / bh as i64, c / bw as i64))
}

/// Memory-mapped block store of one level.
pub struct LevelStore {
    pub lv: Lv,
    map: Option<memmap2::Mmap>,
    index: HashMap<u64, usize>, // key -> byte offset
}

const MAGIC: &[u8; 8] = b"JPDEMLV2";

impl LevelStore {
    pub fn empty(lv: Lv) -> Self {
        LevelStore { lv, map: None, index: HashMap::new() }
    }

    pub fn open(lv: Lv, path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::empty(lv));
        }
        let f = std::fs::File::open(path)?;
        let map = unsafe { memmap2::Mmap::map(&f)? };
        if &map[..8] != MAGIC {
            bail!("bad cache {}", path.display());
        }
        let n = u64::from_le_bytes(map[8..16].try_into()?) as usize;
        let mut index = HashMap::with_capacity(n);
        for i in 0..n {
            let p = 16 + i * 16;
            let k = u64::from_le_bytes(map[p..p + 8].try_into()?);
            let off = u64::from_le_bytes(map[p + 8..p + 16].try_into()?) as usize;
            index.insert(k, off);
        }
        Ok(LevelStore { lv, map: Some(map), index })
    }

    pub fn keys(&self) -> impl Iterator<Item = &u64> {
        self.index.keys()
    }

    #[inline]
    pub fn block(&self, key: u64) -> Option<&[f32]> {
        let off = *self.index.get(&key)?;
        let (bh, bw) = self.lv.block();
        let m = self.map.as_ref()?;
        let bytes = &m[off..off + bh * bw * 4];
        Some(unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f32, bh * bw) })
    }
}

#[cfg(windows)]
pub fn write_at(f: &std::fs::File, buf: &[u8], off: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut done = 0;
    while done < buf.len() {
        done += f.seek_write(&buf[done..], off + done as u64)?;
    }
    Ok(())
}
#[cfg(unix)]
pub fn write_at(f: &std::fs::File, buf: &[u8], off: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    f.write_all_at(buf, off)
}

/// Build a cache file for the selected entries of one level. Every XML must
/// cover exactly one block (true for 1A/5A per 3rd mesh, 10A per 2nd mesh).
pub fn build_cache(lv: Lv, sel: &[DemEntry], path: &Path) -> Result<usize> {
    let es: Vec<&DemEntry> = sel.iter().filter(|e| e.lv == lv).collect();
    let mut keys: Vec<u64> = Vec::new();
    for e in &es {
        keys.push(mesh_block_key(lv, &e.mesh).ok_or_else(|| anyhow!("bad mesh code {}", e.mesh))?);
    }
    keys.sort();
    keys.dedup();
    let (bh, bw) = lv.block();
    let bsize = bh * bw * 4;
    let data0 = (16 + keys.len() * 16).div_ceil(4096) * 4096;
    let mut offs: HashMap<u64, usize> = HashMap::new();
    let tmp = path.with_extension("tmp");
    let f = std::fs::File::create(&tmp)?;
    let mut head = Vec::with_capacity(data0);
    head.extend_from_slice(MAGIC);
    head.extend_from_slice(&(keys.len() as u64).to_le_bytes());
    for (i, k) in keys.iter().enumerate() {
        let off = data0 + i * bsize;
        offs.insert(*k, off);
        head.extend_from_slice(&k.to_le_bytes());
        head.extend_from_slice(&(off as u64).to_le_bytes());
    }
    head.resize(data0, 0);
    f.set_len((data0 + keys.len() * bsize) as u64)?;
    write_at(&f, &head, 0)?;
    let pd = lv.per_deg();
    let errors = std::sync::Mutex::new(Vec::<String>::new());
    let written = std::sync::atomic::AtomicUsize::new(0);
    crate::scan::decode_entries(&es, &|e, grids| {
        for g in grids {
            let row0 = ((ORIGIN_LAT - g.meta.lat_max) * pd).round() as i64;
            let col0 = ((g.meta.lon_min - ORIGIN_LON) * pd).round() as i64;
            if row0 % bh as i64 != 0 || col0 % bw as i64 != 0 || g.meta.nx != bw || g.meta.ny != bh {
                errors.lock().unwrap().push(format!("{} {}: grid not block-aligned", e.inner, e.xml));
                continue;
            }
            let k = bkey(row0 / bh as i64, col0 / bw as i64);
            let Some(&off) = offs.get(&k) else {
                errors.lock().unwrap().push(format!("{} {}: key mismatch", e.inner, e.xml));
                continue;
            };
            let bytes: &[u8] = unsafe { std::slice::from_raw_parts(g.values.as_ptr() as *const u8, bsize) };
            if let Err(err) = write_at(&f, bytes, off as u64) {
                errors.lock().unwrap().push(format!("write: {err}"));
            }
            written.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    })?;
    let errs = errors.into_inner().unwrap();
    if !errs.is_empty() {
        bail!("{} errors, first: {}", errs.len(), errs[0]);
    }
    let n = written.into_inner();
    if n != keys.len() {
        bail!("wrote {} of {} blocks", n, keys.len());
    }
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(keys.len())
}

/// Save in-memory blocks in the cache format (readable by `LevelStore::open`).
pub fn save_blocks(lv: Lv, blocks: &HashMap<u64, Vec<f32>>, path: &Path) -> Result<()> {
    use std::io::Write;
    let mut keys: Vec<u64> = blocks.keys().copied().collect();
    keys.sort();
    let (bh, bw) = lv.block();
    let bsize = bh * bw * 4;
    let data0 = (16 + keys.len() * 16).div_ceil(4096) * 4096;
    let tmp = path.with_extension("tmp");
    let mut f = std::io::BufWriter::with_capacity(1 << 24, std::fs::File::create(&tmp)?);
    let mut head = Vec::with_capacity(data0);
    head.extend_from_slice(MAGIC);
    head.extend_from_slice(&(keys.len() as u64).to_le_bytes());
    for (i, k) in keys.iter().enumerate() {
        head.extend_from_slice(&k.to_le_bytes());
        head.extend_from_slice(&((data0 + i * bsize) as u64).to_le_bytes());
    }
    head.resize(data0, 0);
    f.write_all(&head)?;
    for k in &keys {
        let b = &blocks[k];
        let bytes: &[u8] = unsafe { std::slice::from_raw_parts(b.as_ptr() as *const u8, bsize) };
        f.write_all(bytes)?;
    }
    f.flush()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Result of sampling one level at a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sample {
    Elev(f32),
    Sea,
    Inland,
    None,
}

/// Dense window of blocks for fast lookups inside a lat/lon box.
pub struct Window<'a> {
    br0: i64,
    bc0: i64,
    nbr: i64,
    nbc: i64,
    bh: i64,
    bw: i64,
    blocks: Vec<Option<&'a [f32]>>,
    pd: f64,
}

impl<'a> Window<'a> {
    pub fn new(st: &'a LevelStore, lat_min: f64, lat_max: f64, lon_min: f64, lon_max: f64) -> Self {
        let lv = st.lv;
        let pd = lv.per_deg();
        let (bh, bw) = lv.block();
        let (bh, bw) = (bh as i64, bw as i64);
        let r0 = ((ORIGIN_LAT - lat_max) * pd).floor() as i64 - 2;
        let r1 = ((ORIGIN_LAT - lat_min) * pd).ceil() as i64 + 2;
        let c0 = ((lon_min - ORIGIN_LON) * pd).floor() as i64 - 2;
        let c1 = ((lon_max - ORIGIN_LON) * pd).ceil() as i64 + 2;
        let br0 = r0.div_euclid(bh);
        let bc0 = c0.div_euclid(bw);
        let nbr = r1.div_euclid(bh) - br0 + 1;
        let nbc = c1.div_euclid(bw) - bc0 + 1;
        let mut blocks = Vec::with_capacity((nbr * nbc) as usize);
        for i in 0..nbr {
            for j in 0..nbc {
                blocks.push(st.block(bkey(br0 + i, bc0 + j)));
            }
        }
        Window { br0, bc0, nbr, nbc, bh, bw, blocks, pd }
    }

    #[inline]
    fn cell(&self, r: i64, c: i64) -> f32 {
        let br = r.div_euclid(self.bh) - self.br0;
        let bc = c.div_euclid(self.bw) - self.bc0;
        if br < 0 || bc < 0 || br >= self.nbr || bc >= self.nbc {
            return NODATA;
        }
        match self.blocks[(br * self.nbc + bc) as usize] {
            Some(b) => b[(r.rem_euclid(self.bh) * self.bw + c.rem_euclid(self.bw)) as usize],
            None => NODATA,
        }
    }

    /// Bilinear sample; water/no-data decided by the nearest cell.
    #[inline]
    pub fn sample(&self, lat: f64, lon: f64) -> Sample {
        let fy = (ORIGIN_LAT - lat) * self.pd - 0.5;
        let fx = (lon - ORIGIN_LON) * self.pd - 0.5;
        let r0 = fy.floor();
        let c0 = fx.floor();
        let wy = (fy - r0) as f32;
        let wx = (fx - c0) as f32;
        let (r0, c0) = (r0 as i64, c0 as i64);
        let v00 = self.cell(r0, c0);
        let v01 = self.cell(r0, c0 + 1);
        let v10 = self.cell(r0 + 1, c0);
        let v11 = self.cell(r0 + 1, c0 + 1);
        let near = match (wy >= 0.5, wx >= 0.5) {
            (false, false) => v00,
            (false, true) => v01,
            (true, false) => v10,
            (true, true) => v11,
        };
        if near.is_nan() {
            return Sample::None;
        }
        if !is_elev(near) {
            return if near == gml::SEA { Sample::Sea } else { Sample::Inland };
        }
        let w = [(1.0 - wy) * (1.0 - wx), (1.0 - wy) * wx, wy * (1.0 - wx), wy * wx];
        let v = [v00, v01, v10, v11];
        let mut s = 0.0f32;
        let mut ws = 0.0f32;
        for k in 0..4 {
            if is_elev(v[k]) {
                s += w[k] * v[k];
                ws += w[k];
            }
        }
        Sample::Elev(s / ws)
    }
}
