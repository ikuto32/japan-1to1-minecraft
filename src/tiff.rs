//! Tiled GeoTIFF writer/reader (Deflate, predictor 3 for float / 2 for u8)
//! with the project CRS (JGD2011 / LCC 2SP 30°,40°; origin 36°N 138°30'E)
//! encoded as user-defined GeoKeys. The reader only needs to understand files
//! written by this writer.

use anyhow::{Result, anyhow, bail};
use rayon::prelude::*;
use std::io::{Read, Write};
use std::path::Path;

pub const TILE: usize = 512;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fmt {
    F32,
    U8,
    U16,
    U32,
}

impl Fmt {
    fn bytes(self) -> usize {
        match self {
            Fmt::F32 | Fmt::U32 => 4,
            Fmt::U8 => 1,
            Fmt::U16 => 2,
        }
    }
}

/// Apply the TIFF predictor to one row of raw little-endian samples.
fn predict_row(row: &mut [u8], fmt: Fmt, tmp: &mut Vec<u8>) {
    match fmt {
        Fmt::U16 | Fmt::U32 => {}
        Fmt::U8 => {
            for i in (1..row.len()).rev() {
                row[i] = row[i].wrapping_sub(row[i - 1]);
            }
        }
        Fmt::F32 => {
            // predictor 3: byte planes MSB first, then byte-wise differencing
            let wc = row.len() / 4;
            tmp.clear();
            tmp.extend_from_slice(row);
            for c in 0..wc {
                for b in 0..4 {
                    row[(3 - b) * wc + c] = tmp[4 * c + b];
                }
            }
            for i in (1..row.len()).rev() {
                row[i] = row[i].wrapping_sub(row[i - 1]);
            }
        }
    }
}

fn unpredict_row(row: &mut [u8], fmt: Fmt, tmp: &mut Vec<u8>) {
    if matches!(fmt, Fmt::U16 | Fmt::U32) {
        return;
    }
    for i in 1..row.len() {
        row[i] = row[i].wrapping_add(row[i - 1]);
    }
    if fmt == Fmt::F32 {
        let wc = row.len() / 4;
        tmp.clear();
        tmp.extend_from_slice(row);
        for c in 0..wc {
            for b in 0..4 {
                row[4 * c + b] = tmp[(3 - b) * wc + c];
            }
        }
    }
}

struct Ifd {
    entries: Vec<(u16, u16, u32, Vec<u8>)>, // tag, type, count, payload bytes
}

impl Ifd {
    fn short(&mut self, tag: u16, v: &[u16]) {
        self.entries.push((tag, 3, v.len() as u32, v.iter().flat_map(|x| x.to_le_bytes()).collect()));
    }
    fn long(&mut self, tag: u16, v: &[u32]) {
        self.entries.push((tag, 4, v.len() as u32, v.iter().flat_map(|x| x.to_le_bytes()).collect()));
    }
    fn double(&mut self, tag: u16, v: &[f64]) {
        self.entries.push((tag, 12, v.len() as u32, v.iter().flat_map(|x| x.to_le_bytes()).collect()));
    }
    fn ascii(&mut self, tag: u16, s: &str) {
        let mut b = s.as_bytes().to_vec();
        b.push(0);
        self.entries.push((tag, 2, b.len() as u32, b));
    }
}

/// Write a single-band tiled GeoTIFF. `x0`,`z0`: Minecraft coordinates of the
/// top-left corner of pixel (0,0); pixels are 1 m (easting = X, northing = -Z).
/// Returns false when an identical file already existed (nothing written).
pub fn write(path: &Path, fmt: Fmt, w: usize, h: usize, data: &[u8], x0: i64, z0: i64, nodata: Option<&str>, level: u32) -> Result<bool> {
    let bps = fmt.bytes();
    assert_eq!(data.len(), w * h * bps);
    let tx = w.div_ceil(TILE);
    let ty = h.div_ceil(TILE);
    let tiles: Vec<Vec<u8>> = (0..tx * ty)
        .into_par_iter()
        .map(|t| {
            let (bx, by) = (t % tx, t / tx);
            let mut buf = vec![0u8; TILE * TILE * bps];
            let mut tmp = Vec::new();
            for r in 0..TILE {
                let y = by * TILE + r;
                let row = &mut buf[r * TILE * bps..(r + 1) * TILE * bps];
                if y < h {
                    let xs = bx * TILE;
                    let n = TILE.min(w - xs);
                    row[..n * bps].copy_from_slice(&data[(y * w + xs) * bps..(y * w + xs + n) * bps]);
                }
                predict_row(row, fmt, &mut tmp);
            }
            let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(level));
            e.write_all(&buf).unwrap();
            e.finish().unwrap()
        })
        .collect();
    let mut out: Vec<u8> = Vec::with_capacity(tiles.iter().map(|t| t.len()).sum::<usize>() + 65536);
    out.extend_from_slice(b"II*\0\0\0\0\0");
    let mut offs = Vec::with_capacity(tiles.len());
    let mut counts = Vec::with_capacity(tiles.len());
    for t in &tiles {
        offs.push(out.len() as u32);
        counts.push(t.len() as u32);
        out.extend_from_slice(t);
    }
    if out.len() > u32::MAX as usize - 1_000_000 {
        bail!("TIFF too large for classic format");
    }
    let mut ifd = Ifd { entries: Vec::new() };
    ifd.long(256, &[w as u32]);
    ifd.long(257, &[h as u32]);
    ifd.short(258, &[(bps * 8) as u16]);
    ifd.short(259, &[8]); // deflate
    ifd.short(262, &[1]); // min-is-black
    ifd.short(277, &[1]);
    ifd.short(284, &[1]);
    ifd.short(317, &[match fmt {
        Fmt::F32 => 3,
        Fmt::U8 => 2,
        _ => 1,
    }]);
    ifd.short(322, &[TILE as u16]);
    ifd.short(323, &[TILE as u16]);
    ifd.long(324, &offs);
    ifd.long(325, &counts);
    ifd.short(339, &[if fmt == Fmt::F32 { 3 } else { 1 }]);
    ifd.double(33550, &[1.0, 1.0, 0.0]);
    ifd.double(33922, &[0.0, 0.0, 0.0, x0 as f64, -(z0 as f64), 0.0]);
    let cit = "JGD2011 / LCC 2SP (30N,40N; origin 36N 138.5E; FE=FN=0) - Minecraft 1m grid, X=E, Z=-N|";
    let gcit = "JGD2011|";
    let ascii = format!("{cit}{gcit}");
    // GeoKey directory: header + keys (sorted)
    let mut keys: Vec<[u16; 4]> = vec![
        [1024, 0, 1, 1],                               // GTModelType = projected
        [1025, 0, 1, 1],                               // GTRasterType = PixelIsArea
        [1026, 34737, cit.len() as u16, 0],            // GTCitation
        [2048, 0, 1, 6668],                            // GeographicType = JGD2011
        [2049, 34737, gcit.len() as u16, cit.len() as u16], // GeogCitation
        [2054, 0, 1, 9102],                            // angular unit degree
        [3072, 0, 1, 32767],                           // ProjectedCSType user-defined
        [3074, 0, 1, 32767],                           // Projection user-defined
        [3075, 0, 1, 8],                               // CT_LambertConfConic_2SP
        [3076, 0, 1, 9001],                            // metre
        [3078, 34736, 1, 0],                           // StdParallel1
        [3079, 34736, 1, 1],                           // StdParallel2
        [3084, 34736, 1, 2],                           // FalseOriginLong
        [3085, 34736, 1, 3],                           // FalseOriginLat
        [3086, 34736, 1, 4],                           // FalseOriginEasting
        [3087, 34736, 1, 5],                           // FalseOriginNorthing
    ];
    keys.sort_by_key(|k| k[0]);
    let mut dir: Vec<u16> = vec![1, 1, 0, keys.len() as u16];
    for k in &keys {
        dir.extend_from_slice(k);
    }
    ifd.short(34735, &dir);
    ifd.double(34736, &[crate::proj::LAT1, crate::proj::LAT2, crate::proj::LON0, crate::proj::LAT0, 0.0, 0.0]);
    ifd.ascii(34737, &ascii);
    if let Some(nd) = nodata {
        ifd.ascii(42113, nd);
    }
    ifd.entries.sort_by_key(|e| e.0);
    // layout: IFD then out-of-line payloads
    if out.len() % 2 == 1 {
        out.push(0);
    }
    let ifd_off = out.len();
    let n = ifd.entries.len();
    let mut extra_off = ifd_off + 2 + n * 12 + 4;
    let mut extra: Vec<u8> = Vec::new();
    let mut ent: Vec<u8> = Vec::new();
    ent.extend_from_slice(&(n as u16).to_le_bytes());
    for (tag, ty, count, payload) in &ifd.entries {
        ent.extend_from_slice(&tag.to_le_bytes());
        ent.extend_from_slice(&ty.to_le_bytes());
        ent.extend_from_slice(&count.to_le_bytes());
        if payload.len() <= 4 {
            let mut v = payload.clone();
            v.resize(4, 0);
            ent.extend_from_slice(&v);
        } else {
            ent.extend_from_slice(&(extra_off as u32).to_le_bytes());
            extra.extend_from_slice(payload);
            extra_off += payload.len();
            if extra_off % 2 == 1 {
                extra.push(0);
                extra_off += 1;
            }
        }
    }
    ent.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&ent);
    out.extend_from_slice(&extra);
    out[4..8].copy_from_slice(&(ifd_off as u32).to_le_bytes());
    // leave identical files untouched (keeps their mtime for incremental world builds)
    if let Ok(old) = std::fs::read(path) {
        if old == out {
            return Ok(false);
        }
    }
    let tmp = path.with_extension("tif.tmp");
    std::fs::write(&tmp, &out)?;
    std::fs::rename(&tmp, path)?;
    Ok(true)
}

/// Reader for files produced by `write`.
pub struct Reader {
    data: memmap2::Mmap,
    pub w: usize,
    pub h: usize,
    pub fmt: Fmt,
    offs: Vec<u32>,
    counts: Vec<u32>,
}

impl Reader {
    pub fn open(path: &Path) -> Result<Self> {
        let f = std::fs::File::open(path)?;
        let data = unsafe { memmap2::Mmap::map(&f)? };
        if &data[..4] != b"II*\0" {
            bail!("not a little-endian TIFF: {}", path.display());
        }
        let ifd = u32::from_le_bytes(data[4..8].try_into()?) as usize;
        let n = u16::from_le_bytes(data[ifd..ifd + 2].try_into()?) as usize;
        let (mut w, mut h, mut bps, mut offs, mut counts) = (0, 0, 0, Vec::new(), Vec::new());
        let mut sfmt = 1;
        for i in 0..n {
            let e = ifd + 2 + i * 12;
            let tag = u16::from_le_bytes(data[e..e + 2].try_into()?);
            let ty = u16::from_le_bytes(data[e + 2..e + 4].try_into()?);
            let count = u32::from_le_bytes(data[e + 4..e + 8].try_into()?) as usize;
            let size = match ty {
                3 => 2,
                4 => 4,
                12 => 8,
                _ => 1,
            } * count;
            let p = if size <= 4 { e + 8 } else { u32::from_le_bytes(data[e + 8..e + 12].try_into()?) as usize };
            let vals = |d: &[u8]| -> Vec<u32> {
                (0..count)
                    .map(|k| match ty {
                        3 => u16::from_le_bytes(d[p + 2 * k..p + 2 * k + 2].try_into().unwrap()) as u32,
                        _ => u32::from_le_bytes(d[p + 4 * k..p + 4 * k + 4].try_into().unwrap()),
                    })
                    .collect()
            };
            match tag {
                256 => w = vals(&data)[0] as usize,
                257 => h = vals(&data)[0] as usize,
                258 => bps = vals(&data)[0] as usize,
                324 => offs = vals(&data),
                325 => counts = vals(&data),
                339 => sfmt = vals(&data)[0],
                _ => {}
            }
        }
        let fmt = match (bps, sfmt) {
            (32, 3) => Fmt::F32,
            (32, _) => Fmt::U32,
            (16, _) => Fmt::U16,
            _ => Fmt::U8,
        };
        if offs.is_empty() {
            return Err(anyhow!("no tiles in {}", path.display()));
        }
        Ok(Reader { data, w, h, fmt, offs, counts })
    }

    /// Decode one 512×512 tile (raw little-endian sample bytes).
    pub fn tile(&self, tx: usize, ty: usize) -> Result<Vec<u8>> {
        let ntx = self.w.div_ceil(TILE);
        if tx >= ntx || ty >= self.h.div_ceil(TILE) {
            bail!("tile ({tx},{ty}) outside image");
        }
        let i = ty * ntx + tx;
        let (o, c) = (self.offs[i] as usize, self.counts[i] as usize);
        let mut out = Vec::with_capacity(TILE * TILE * self.fmt.bytes());
        flate2::read::ZlibDecoder::new(&self.data[o..o + c]).read_to_end(&mut out)?;
        let rb = TILE * self.fmt.bytes();
        let mut tmp = Vec::new();
        for r in 0..TILE {
            unpredict_row(&mut out[r * rb..(r + 1) * rb], self.fmt, &mut tmp);
        }
        Ok(out)
    }

}

pub fn f32_bytes(v: &[f32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 4) }
}

pub fn bytes_to_f32(b: Vec<u8>) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
}
