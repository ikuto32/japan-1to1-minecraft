//! Lake-bed depth grids (国土地理院 湖沼データ, 10 m grid CSV: X = northing,
//! Y = easting in plane rectangular zone VIII, DEPTH in m below the lake
//! surface, -9999 outside). Sampled bilinearly in Minecraft coordinates.

use crate::proj::{Lcc, Tm};
use anyhow::Result;
use std::io::Read;
use std::path::Path;

pub struct Grid {
    pub name: String,
    e0: f64,
    n0: f64,
    step: f64,
    cols: usize,
    rows: usize,
    depth: Vec<f32>, // row-major, row = northing index from n0
    // MC = o + A·(e - e0, n - n0); inverse kept for sampling
    o: (f64, f64),
    inv: [f64; 4],
    pub bbox: (f64, f64, f64, f64), // MC x0, z0, x1, z1
}

impl Grid {
    /// Depth (m) at a Minecraft position, NaN outside the surveyed lake.
    pub fn sample(&self, x: f64, z: f64) -> f32 {
        let (dx, dz) = (x - self.o.0, z - self.o.1);
        let de = self.inv[0] * dx + self.inv[1] * dz;
        let dn = self.inv[2] * dx + self.inv[3] * dz;
        let (fc, fr) = (de / self.step, dn / self.step);
        let (c0, r0) = (fc.floor(), fr.floor());
        let (tc, tr) = ((fc - c0) as f32, (fr - r0) as f32);
        let (c0, r0) = (c0 as i64, r0 as i64);
        let (mut s, mut w) = (0f32, 0f32);
        for (dc, dr, wt) in [(0, 0, (1.0 - tc) * (1.0 - tr)), (1, 0, tc * (1.0 - tr)), (0, 1, (1.0 - tc) * tr), (1, 1, tc * tr)] {
            let (c, r) = (c0 + dc, r0 + dr);
            if c < 0 || r < 0 || c >= self.cols as i64 || r >= self.rows as i64 {
                continue;
            }
            let v = self.depth[r as usize * self.cols + c as usize];
            if v.is_finite() && wt > 0.0 {
                s += v * wt;
                w += wt;
            }
        }
        if w > 0.05 { s / w } else { f32::NAN }
    }
}

fn parse_csv(name: &str, text: &str, tm: &Tm, lcc: &Lcc) -> Option<Grid> {
    let mut pts: Vec<(f64, f64, f32)> = Vec::new();
    for l in text.lines().skip(1) {
        let c: Vec<&str> = l.split(',').collect();
        if c.len() < 3 {
            continue;
        }
        let (Ok(n), Ok(e), Ok(d)) = (c[0].trim().parse::<f64>(), c[1].trim().parse::<f64>(), c[2].trim().parse::<f32>()) else { continue };
        pts.push((e, n, if d < -9000.0 { f32::NAN } else { d }));
    }
    if pts.is_empty() {
        return None;
    }
    let e0 = pts.iter().map(|p| p.0).fold(f64::MAX, f64::min);
    let n0 = pts.iter().map(|p| p.1).fold(f64::MAX, f64::min);
    let e1 = pts.iter().map(|p| p.0).fold(f64::MIN, f64::max);
    let n1 = pts.iter().map(|p| p.1).fold(f64::MIN, f64::max);
    let step = 10.0;
    let cols = ((e1 - e0) / step).round() as usize + 1;
    let rows = ((n1 - n0) / step).round() as usize + 1;
    let mut depth = vec![f32::NAN; cols * rows];
    for (e, n, d) in pts {
        let (c, r) = (((e - e0) / step).round() as usize, ((n - n0) / step).round() as usize);
        depth[r * cols + c] = d;
    }
    let mc = |e: f64, n: f64| {
        let (la, lo) = tm.inverse(e, n);
        lcc.ll_to_mc(la, lo)
    };
    let o = mc(e0, n0);
    let pe = mc(e0 + 1000.0, n0);
    let pn = mc(e0, n0 + 1000.0);
    let a = [(pe.0 - o.0) / 1000.0, (pn.0 - o.0) / 1000.0, (pe.1 - o.1) / 1000.0, (pn.1 - o.1) / 1000.0];
    let det = a[0] * a[3] - a[1] * a[2];
    let inv = [a[3] / det, -a[1] / det, -a[2] / det, a[0] / det];
    let corners = [o, mc(e1, n0), mc(e0, n1), mc(e1, n1)];
    let bbox = (
        corners.iter().map(|c| c.0).fold(f64::MAX, f64::min),
        corners.iter().map(|c| c.1).fold(f64::MAX, f64::min),
        corners.iter().map(|c| c.0).fold(f64::MIN, f64::max),
        corners.iter().map(|c| c.1).fold(f64::MIN, f64::max),
    );
    Some(Grid { name: name.to_string(), e0, n0, step, cols, rows, depth, o, inv, bbox })
}

/// All lakes under `dir` (one zip per lake; the newest `*_10m.csv` inside wins).
pub fn load(dir: &Path) -> Result<Vec<Grid>> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return Ok(out) };
    let tm = Tm::zone8();
    let lcc = Lcc::new();
    let mut zips: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip"))).collect();
    zips.sort();
    for z in zips {
        let mut za = zip::ZipArchive::new(std::io::BufReader::new(std::fs::File::open(&z)?))?;
        let mut best: Option<(String, usize)> = None;
        for i in 0..za.len() {
            let n = za.by_index_raw(i)?.name().to_string();
            if n.ends_with("_10m.csv") {
                let file = n.rsplit('/').next().unwrap_or(&n).to_string();
                if best.as_ref().is_none_or(|b| file > b.0) {
                    best = Some((file, i));
                }
            }
        }
        let Some((file, i)) = best else { continue };
        let mut d = Vec::new();
        za.by_index(i)?.read_to_end(&mut d)?;
        let text = encoding_rs::SHIFT_JIS.decode(&d).0.into_owned();
        if let Some(g) = parse_csv(&file, &text, &tm, &lcc) {
            let valid = g.depth.iter().filter(|v| v.is_finite()).count();
            let maxd = g.depth.iter().copied().filter(|v| v.is_finite()).fold(0f32, f32::max);
            eprintln!("bathymetry: {file}: {}x{} cells, {valid} with depth (max {maxd:.1} m)", g.cols, g.rows);
            let _ = (g.e0, g.n0);
            out.push(g);
        }
    }
    Ok(out)
}
