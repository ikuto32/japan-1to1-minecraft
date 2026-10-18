//! Meta / WRI High Resolution Canopy Height Maps v2 (CHMv2): canopy height
//! from satellite imagery, about 1 m, in Web Mercator z10 tiles (32768² uint8
//! metres, page 1 = the validity mask). `ingest` resamples them onto the
//! heightmap tile grid as work/trees/chm2_*.tif (canopy height in metres, as
//! mapped) and chm2cov_*.tif (1 where the map has data); the features stage
//! calibrates the heights and finds the trees in them (see `calibrate`).

use crate::heightmap::TILE;
use crate::proj::{Lcc, LlGrid};
use crate::tiff::{self, Reader};
use anyhow::Result;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const R: f64 = 6378137.0; // Web Mercator sphere
const B: usize = tiff::TILE; // 512

struct Src {
    data: Reader,
    mask: Reader,
    /// x, y of the top-left corner, pixel size (EPSG:3857 m)
    x0: f64,
    y0: f64,
    sx: f64,
    sy: f64,
}

impl Src {
    fn contains(&self, mx: f64, my: f64) -> bool {
        mx >= self.x0 && my <= self.y0 && mx < self.x0 + self.data.w as f64 * self.sx && my > self.y0 - self.data.h as f64 * self.sy
    }
}

fn tile_name(prefix: &str, tx: i64, tz: i64) -> String {
    format!("{prefix}_X{:+07}_Z{:+07}.tif", tx * TILE, tz * TILE)
}

/// Resample every CHMv2 tile under `dir` (its `chm` folder) onto the
/// heightmap tiles (`hm_dir`) they touch.
pub fn ingest(dir: &Path, work: &Path, hm_dir: &Path) -> Result<()> {
    let t0 = std::time::Instant::now();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join("chm"))?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "tif")).collect();
    files.sort();
    let mut srcs: Vec<Src> = Vec::new();
    for p in &files {
        let data = Reader::open_page(p, 0)?;
        let mask = Reader::open_page(p, 1)?;
        let Some([x0, y0, sx, sy]) = data.geo else { anyhow::bail!("{}: no GeoTIFF placement", p.display()) };
        srcs.push(Src { data, mask, x0, y0, sx, sy });
    }
    eprintln!("chm2: {} CHMv2 tiles", srcs.len());
    // heightmap tiles
    let mut tiles: Vec<(i64, i64)> = Vec::new();
    for e in std::fs::read_dir(hm_dir)? {
        let name = e?.file_name().to_string_lossy().to_string();
        if let Some(rest) = name.strip_prefix("height_X").and_then(|s| s.strip_suffix(".tif")) {
            if let Some((xs, zs)) = rest.split_once("_Z") {
                if let (Ok(x), Ok(z)) = (xs.parse::<i64>(), zs.parse::<i64>()) {
                    tiles.push((x / TILE, z / TILE));
                }
            }
        }
    }
    tiles.sort_by_key(|t| (t.1, t.0));
    let lcc = Lcc::new();
    let out = work.join("trees");
    std::fs::create_dir_all(&out)?;
    let t = TILE as usize;
    let done = std::sync::atomic::AtomicUsize::new(0);
    let written = std::sync::atomic::AtomicUsize::new(0);
    for (tx, tz) in tiles.iter().copied() {
        let (x0, z0) = (tx * TILE, tz * TILE);
        let ll = LlGrid::new(&lcc, x0, z0, TILE, TILE);
        let mut chm = vec![0u8; t * t];
        let mut cov = vec![0u8; t * t];
        // strips of 512 rows, each with its own cache of decoded source blocks
        chm.par_chunks_mut(t * B).zip(cov.par_chunks_mut(t * B)).enumerate().for_each(|(s, (crow, vrow))| {
            let mut cache: HashMap<(usize, usize, usize), Option<Arc<(Vec<u8>, Vec<u8>)>>> = HashMap::new();
            let mut last = usize::MAX;
            for r in 0..crow.len() / t {
                let z = z0 + (s * B + r) as i64;
                for c in 0..t {
                    let (la, lo) = ll.get(x0 + c as i64, z);
                    let mx = R * lo.to_radians();
                    let my = R * (std::f64::consts::FRAC_PI_4 + la.to_radians() / 2.0).tan().ln();
                    let si = if last != usize::MAX && srcs[last].contains(mx, my) {
                        last
                    } else {
                        match srcs.iter().position(|q| q.contains(mx, my)) {
                            Some(i) => i,
                            None => continue,
                        }
                    };
                    last = si;
                    let q = &srcs[si];
                    let (px, py) = (((mx - q.x0) / q.sx) as usize, ((q.y0 - my) / q.sy) as usize);
                    let (bx, by) = (px / B, py / B);
                    let blk = cache
                        .entry((si, bx, by))
                        .or_insert_with(|| match (q.data.tile(bx, by), q.mask.tile(bx, by)) {
                            (Ok(d), Ok(m)) => Some(Arc::new((d, m))),
                            _ => None,
                        })
                        .clone();
                    let Some(blk) = blk else { continue };
                    let (lx, ly) = (px % B, py % B);
                    if (blk.1[ly * (B / 8) + lx / 8] >> (7 - lx % 8)) & 1 == 1 {
                        crow[r * t + c] = blk.0[ly * B + lx];
                        vrow[r * t + c] = 1;
                    }
                }
            }
        });
        let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let n = cov.iter().filter(|&&v| v != 0).count();
        if n > 0 {
            tiff::write(&out.join(tile_name("chm2", tx, tz)), tiff::Fmt::U8, t, t, &chm, x0, z0, Some("0"), 6)?;
            tiff::write(&out.join(tile_name("chm2cov", tx, tz)), tiff::Fmt::U8, t, t, &cov, x0, z0, Some("0"), 6)?;
            written.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if d % 20 == 0 || d == tiles.len() {
            eprintln!("  chm2: {d}/{} tiles ({} with data) {:.0}s", tiles.len(), written.load(std::sync::atomic::Ordering::Relaxed), t0.elapsed().as_secs_f64());
        }
    }
    eprintln!("chm2: done in {:.0}s", t0.elapsed().as_secs_f64());
    Ok(())
}

/// CHMv2 layers of a heightmap tile (None where there is none).
pub fn read(work: &Path, tx: i64, tz: i64) -> Option<(Vec<u8>, Vec<u8>)> {
    let rd = |name: &str| -> Option<Vec<u8>> {
        let r = Reader::open(&work.join("trees").join(tile_name(name, tx, tz))).ok()?;
        let t = TILE as usize;
        let mut v = vec![0u8; t * t];
        for by in 0..t / B {
            for bx in 0..t / B {
                let b = r.tile(bx, by).ok()?;
                for row in 0..B {
                    let d = (by * B + row) * t + bx * B;
                    v[d..d + B].copy_from_slice(&b[row * B..row * B + B]);
                }
            }
        }
        Some(v)
    };
    Some((rd("chm2")?, rd("chm2cov")?))
}
