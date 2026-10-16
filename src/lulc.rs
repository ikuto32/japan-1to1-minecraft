//! JAXA 高解像度土地利用土地被覆図 (日本域) — 1°×1° GeoTIFF tiles
//! (12000×12000, 0.3″ pixels, WGS84 ≈ JGD2011) read straight from the zip.

use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

pub struct Lulc {
    /// (lat_floor, lon_floor) -> 12000×12000 class codes, row 0 = north
    tiles: HashMap<(i32, i32), Vec<u8>>,
    pub n: usize,
}

impl Lulc {
    /// Load the tiles covering lat [lat0, lat1) × lon [lon0, lon1).
    pub fn load(zip_path: &Path, lat0: i32, lat1: i32, lon0: i32, lon1: i32) -> Result<Self> {
        let f = std::fs::File::open(zip_path).with_context(|| format!("{}", zip_path.display()))?;
        let mut za = zip::ZipArchive::new(std::io::BufReader::new(f))?;
        let names: Vec<String> = za.file_names().map(|s| s.to_string()).collect();
        let mut tiles = HashMap::new();
        for la in lat0..lat1 {
            for lo in lon0..lon1 {
                let want = format!("LC_N{la}E{lo}.tif");
                let Some(n) = names.iter().find(|n| n.ends_with(&want)) else { continue };
                let mut buf = Vec::new();
                za.by_name(n)?.read_to_end(&mut buf)?;
                let (w, h, v) = decode_strips_u8(&buf).with_context(|| n.clone())?;
                if (w, h) != (12000, 12000) {
                    bail!("{n}: unexpected size {w}x{h}");
                }
                tiles.insert((la, lo), v);
            }
        }
        let n = tiles.len();
        Ok(Lulc { tiles, n })
    }

}

/// Minimal reader for 8-bit, strip-organised, little-endian TIFFs with
/// no / deflate compression and optional horizontal predictor (palette
/// images included — the palette is ignored, values are class codes).
fn decode_strips_u8(d: &[u8]) -> Result<(usize, usize, Vec<u8>)> {
    if d.len() < 8 || &d[..4] != b"II*\0" {
        bail!("not a little-endian classic TIFF");
    }
    let u16_ = |p: usize| u16::from_le_bytes([d[p], d[p + 1]]) as usize;
    let u32_ = |p: usize| u32::from_le_bytes([d[p], d[p + 1], d[p + 2], d[p + 3]]) as usize;
    let ifd = u32_(4);
    let n = u16_(ifd);
    let (mut w, mut h, mut rps, mut comp, mut pred, mut bps) = (0, 0, 0, 1, 1, 8);
    let (mut offs, mut counts) = (Vec::new(), Vec::new());
    for i in 0..n {
        let e = ifd + 2 + i * 12;
        let (tag, ty, cnt) = (u16_(e), u16_(e + 2), u32_(e + 4));
        let size = if ty == 3 { 2 } else { 4 } * cnt;
        let p = if size <= 4 { e + 8 } else { u32_(e + 8) };
        let vals = |k: usize| if ty == 3 { u16_(p + 2 * k) } else { u32_(p + 4 * k) };
        match tag {
            256 => w = vals(0),
            257 => h = vals(0),
            258 => bps = vals(0),
            259 => comp = vals(0),
            273 => offs = (0..cnt).map(vals).collect(),
            278 => rps = vals(0),
            279 => counts = (0..cnt).map(vals).collect(),
            317 => pred = vals(0),
            _ => {}
        }
    }
    if bps != 8 {
        bail!("{bps} bits per sample");
    }
    let rps = if rps == 0 { h } else { rps };
    let mut out = vec![0u8; w * h];
    for (si, (&o, &c)) in offs.iter().zip(&counts).enumerate() {
        let raw = &d[o..o + c];
        let mut strip = Vec::with_capacity(rps * w);
        match comp {
            1 => strip.extend_from_slice(raw),
            8 | 32946 => {
                flate2::read::ZlibDecoder::new(raw).read_to_end(&mut strip)?;
            }
            _ => bail!("compression {comp} not supported"),
        }
        let r0 = si * rps;
        let rows = rps.min(h - r0);
        for r in 0..rows {
            let row = &mut strip[r * w..(r + 1) * w];
            if pred == 2 {
                for x in 1..w {
                    row[x] = row[x].wrapping_add(row[x - 1]);
                }
            }
            out[(r0 + r) * w..(r0 + r + 1) * w].copy_from_slice(row);
        }
    }
    Ok((w, h, out))
}

impl Lulc {
    /// Class code at (lat, lon), 0 = no data.
    #[inline]
    pub fn at(&self, lat: f64, lon: f64) -> u8 {
        let (la, lo) = (lat.floor() as i32, lon.floor() as i32);
        let Some(t) = self.tiles.get(&(la, lo)) else { return 0 };
        let r = (((la + 1) as f64 - lat) * 12000.0) as usize;
        let c = ((lon - lo as f64) * 12000.0) as usize;
        t[r.min(11999) * 12000 + c.min(11999)]
    }
}
