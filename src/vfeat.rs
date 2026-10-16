//! Vector features in Minecraft coordinates (X = easting, Z = -northing),
//! stored in one file per layer and bucketed by heightmap tile (10240 m) so
//! the per-tile stages only read what they need.

use crate::heightmap::TILE;
use anyhow::{Result, bail};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct VFeat {
    /// layer-specific class code
    pub class: u16,
    /// layer-specific bit flags (bridge, tunnel, area, ...)
    pub flags: u16,
    /// numeric attributes (meaning depends on the layer: width, levels, height, layer ...)
    pub nums: [f32; 4],
    pub id: u64,
    /// free text (type name, tags) — keep short
    pub text: String,
    /// polygon rings (first = exterior) when `area`, else polylines
    pub area: bool,
    pub parts: Vec<Vec<[f64; 2]>>,
    /// optional per-vertex values parallel to `parts` (e.g. surface heights)
    pub zs: Vec<Vec<f32>>,
}

impl VFeat {
    pub fn bbox(&self) -> (f64, f64, f64, f64) {
        let (mut x0, mut z0, mut x1, mut z1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in &self.parts {
            for &[x, z] in p {
                x0 = x0.min(x);
                z0 = z0.min(z);
                x1 = x1.max(x);
                z1 = z1.max(z);
            }
        }
        (x0, z0, x1, z1)
    }

    fn encode(&self, b: &mut Vec<u8>) {
        b.extend_from_slice(&self.class.to_le_bytes());
        b.extend_from_slice(&self.flags.to_le_bytes());
        for v in self.nums {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&self.id.to_le_bytes());
        b.push(self.area as u8);
        let t = self.text.as_bytes();
        b.extend_from_slice(&(t.len() as u16).to_le_bytes());
        b.extend_from_slice(t);
        b.extend_from_slice(&(self.parts.len() as u32).to_le_bytes());
        for p in &self.parts {
            b.extend_from_slice(&(p.len() as u32).to_le_bytes());
            for &[x, z] in p {
                b.extend_from_slice(&x.to_le_bytes());
                b.extend_from_slice(&z.to_le_bytes());
            }
        }
        b.extend_from_slice(&(self.zs.len() as u32).to_le_bytes());
        for zz in &self.zs {
            b.extend_from_slice(&(zz.len() as u32).to_le_bytes());
            for v in zz {
                b.extend_from_slice(&v.to_le_bytes());
            }
        }
    }

    fn decode(d: &[u8], p: &mut usize) -> VFeat {
        let u16_ = |p: &mut usize| {
            let v = u16::from_le_bytes(d[*p..*p + 2].try_into().unwrap());
            *p += 2;
            v
        };
        let u32_ = |p: &mut usize| {
            let v = u32::from_le_bytes(d[*p..*p + 4].try_into().unwrap());
            *p += 4;
            v
        };
        let f64_ = |p: &mut usize| {
            let v = f64::from_le_bytes(d[*p..*p + 8].try_into().unwrap());
            *p += 8;
            v
        };
        let class = u16_(p);
        let flags = u16_(p);
        let mut nums = [0f32; 4];
        for n in nums.iter_mut() {
            *n = f32::from_le_bytes(d[*p..*p + 4].try_into().unwrap());
            *p += 4;
        }
        let id = u64::from_le_bytes(d[*p..*p + 8].try_into().unwrap());
        *p += 8;
        let area = d[*p] != 0;
        *p += 1;
        let tl = u16_(p) as usize;
        let text = String::from_utf8_lossy(&d[*p..*p + tl]).into_owned();
        *p += tl;
        let np = u32_(p) as usize;
        let mut parts = Vec::with_capacity(np);
        for _ in 0..np {
            let n = u32_(p) as usize;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                let x = f64_(p);
                let z = f64_(p);
                v.push([x, z]);
            }
            parts.push(v);
        }
        let nz = u32_(p) as usize;
        let mut zs = Vec::with_capacity(nz);
        for _ in 0..nz {
            let n = u32_(p) as usize;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(f32::from_le_bytes(d[*p..*p + 4].try_into().unwrap()));
                *p += 4;
            }
            zs.push(v);
        }
        VFeat { class, flags, nums, id, text, area, parts, zs }
    }
}

const MAGIC: &[u8; 8] = b"JPVFEAT2";

/// Write features bucketed by tile. A feature goes to every tile its bbox
/// (grown by `margin` metres) touches.
pub fn write_tiled(path: &Path, feats: &[VFeat], margin: f64) -> Result<()> {
    let mut buckets: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, f) in feats.iter().enumerate() {
        if f.parts.is_empty() {
            continue;
        }
        let (x0, z0, x1, z1) = f.bbox();
        let t = |v: f64| (v.floor() as i64).div_euclid(TILE);
        for tz in t(z0 - margin)..=t(z1 + margin) {
            for tx in t(x0 - margin)..=t(x1 + margin) {
                buckets.entry((tx, tz)).or_default().push(i);
            }
        }
    }
    let mut keys: Vec<(i64, i64)> = buckets.keys().copied().collect();
    keys.sort();
    let tmp = path.with_extension("tmp");
    let mut f = std::io::BufWriter::with_capacity(1 << 24, std::fs::File::create(&tmp)?);
    let head = 16 + keys.len() * 32;
    let mut index = Vec::with_capacity(keys.len());
    let mut body: Vec<u8> = Vec::new();
    let mut off = head as u64;
    // encode tile by tile to keep memory bounded
    f.write_all(MAGIC)?;
    f.write_all(&(keys.len() as u64).to_le_bytes())?;
    let mut blobs: Vec<Vec<u8>> = Vec::with_capacity(keys.len());
    for k in &keys {
        body.clear();
        let ids = &buckets[k];
        for &i in ids {
            feats[i].encode(&mut body);
        }
        index.push((k.0, k.1, off, ids.len() as u32, body.len() as u64));
        off += body.len() as u64;
        blobs.push(std::mem::take(&mut body));
    }
    for (tx, tz, o, n, l) in &index {
        f.write_all(&(*tx as i32).to_le_bytes())?;
        f.write_all(&(*tz as i32).to_le_bytes())?;
        f.write_all(&o.to_le_bytes())?;
        f.write_all(&(*n as u64).to_le_bytes())?;
        f.write_all(&l.to_le_bytes())?;
    }
    for b in blobs {
        f.write_all(&b)?;
    }
    f.flush()?;
    drop(f);
    // keep the old file (and its modification time) when nothing changed, so
    // the incremental stages downstream don't see a new input
    if same_content(&tmp, path)? {
        std::fs::remove_file(&tmp)?;
    } else {
        std::fs::rename(&tmp, path)?;
    }
    Ok(())
}

fn same_content(a: &Path, b: &Path) -> Result<bool> {
    use std::io::Read;
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else { return Ok(false) };
    if ma.len() != mb.len() {
        return Ok(false);
    }
    let (mut fa, mut fb) = (std::io::BufReader::with_capacity(1 << 22, std::fs::File::open(a)?), std::io::BufReader::with_capacity(1 << 22, std::fs::File::open(b)?));
    let (mut ba, mut bb) = (vec![0u8; 1 << 22], vec![0u8; 1 << 22]);
    loop {
        let n = fa.read(&mut ba)?;
        if n == 0 {
            return Ok(true);
        }
        fb.read_exact(&mut bb[..n])?;
        if ba[..n] != bb[..n] {
            return Ok(false);
        }
    }
}

pub struct TiledReader {
    map: Option<memmap2::Mmap>,
    index: HashMap<(i64, i64), (usize, usize, usize)>, // offset, count, bytes
}

impl TiledReader {
    /// Missing file = empty layer.
    pub fn open(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(TiledReader { map: None, index: HashMap::new() });
        }
        let f = std::fs::File::open(path)?;
        let map = unsafe { memmap2::Mmap::map(&f)? };
        if &map[..8] != MAGIC {
            bail!("bad vector file {}", path.display());
        }
        let n = u64::from_le_bytes(map[8..16].try_into()?) as usize;
        let mut index = HashMap::with_capacity(n);
        for i in 0..n {
            let p = 16 + i * 32;
            let tx = i32::from_le_bytes(map[p..p + 4].try_into()?) as i64;
            let tz = i32::from_le_bytes(map[p + 4..p + 8].try_into()?) as i64;
            let o = u64::from_le_bytes(map[p + 8..p + 16].try_into()?) as usize;
            let c = u64::from_le_bytes(map[p + 16..p + 24].try_into()?) as usize;
            let l = u64::from_le_bytes(map[p + 24..p + 32].try_into()?) as usize;
            index.insert((tx, tz), (o, c, l));
        }
        Ok(TiledReader { map: Some(map), index })
    }

    pub fn tile(&self, tx: i64, tz: i64) -> Vec<VFeat> {
        let (Some(m), Some(&(o, c, _))) = (self.map.as_ref(), self.index.get(&(tx, tz))) else { return Vec::new() };
        let mut p = o;
        (0..c).map(|_| VFeat::decode(m, &mut p)).collect()
    }

    pub fn tiles(&self) -> Vec<(i64, i64)> {
        let mut v: Vec<(i64, i64)> = self.index.keys().copied().collect();
        v.sort();
        v
    }
}
