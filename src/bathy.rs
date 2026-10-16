//! Lake-bed depth grids (国土地理院 湖沼データ, 10 m grid CSV: X = northing,
//! Y = easting in the plane rectangular zone named by the .prj in the zip —
//! VIII for the Fuji lakes, IX for 芦ノ湖 —, DEPTH in m below the lake
//! surface, -9999 outside), and the sea floor (ETOPO 2022, 15″ grid).
//! Sampled bilinearly in Minecraft coordinates.

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
        // plane rectangular zone of the grid (from any .prj in the archive)
        let mut tm: Option<Tm> = None;
        for j in 0..za.len() {
            let n = za.by_index_raw(j)?.name().to_string();
            if n.to_ascii_lowercase().ends_with(".prj") {
                let mut p = String::new();
                za.by_index(j)?.read_to_string(&mut p)?;
                tm = zone_of_prj(&p);
                if tm.is_some() {
                    break;
                }
            }
        }
        let tm = tm.unwrap_or_else(Tm::zone8);
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

/// Plane rectangular zone (JGD2011, zones I–XIII) named in a .prj file.
fn zone_of_prj(prj: &str) -> Option<Tm> {
    let i = prj.find("Plane_Rectangular_CS_")? + "Plane_Rectangular_CS_".len();
    let roman: String = prj[i..].chars().take_while(|c| matches!(c, 'I' | 'V' | 'X')).collect();
    let origin = match roman.as_str() {
        "I" => (33.0, 129.5),
        "II" => (33.0, 131.0),
        "III" => (36.0, 132.0 + 10.0 / 60.0),
        "IV" => (33.0, 133.5),
        "V" => (36.0, 134.0 + 20.0 / 60.0),
        "VI" => (36.0, 136.0),
        "VII" => (36.0, 137.0 + 10.0 / 60.0),
        "VIII" => (36.0, 138.5),
        "IX" => (36.0, 139.0 + 50.0 / 60.0),
        "X" => (40.0, 140.0 + 50.0 / 60.0),
        "XI" => (44.0, 140.25),
        "XII" => (44.0, 142.25),
        "XIII" => (44.0, 144.25),
        _ => return None,
    };
    Some(Tm::new(origin.0, origin.1))
}

/// Sea-floor elevation from ETOPO 2022 15″ GeoTIFF tiles (m above mean sea
/// level, negative below it), cropped to the area and sampled bilinearly in
/// latitude / longitude between pixel centres.
pub struct Sea {
    lat0: f64, // latitude of the centre of row 0 (north edge)
    lon0: f64, // longitude of the centre of column 0
    step: f64, // degrees per pixel
    w: usize,
    h: usize,
    z: Vec<f32>,
}

impl Sea {
    /// All `*.tif` under `dir` except the source-id grids, cropped to the box.
    pub fn load(dir: &Path, lat_min: f64, lat_max: f64, lon_min: f64, lon_max: f64) -> Result<Option<Sea>> {
        let Ok(rd) = std::fs::read_dir(dir) else { return Ok(None) };
        let mut files: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let n = p.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
                n.ends_with(".tif") && !n.contains("_sid")
            })
            .collect();
        files.sort();
        let mut out: Option<Sea> = None;
        for f in files {
            let mut dec = tiff::decoder::Decoder::new(std::io::BufReader::new(std::fs::File::open(&f)?))?.with_limits(tiff::decoder::Limits::unlimited());
            let (w, h) = dec.dimensions()?;
            let scale = dec.get_tag_f64_vec(tiff::tags::Tag::ModelPixelScaleTag)?;
            let tie = dec.get_tag_f64_vec(tiff::tags::Tag::ModelTiepointTag)?;
            // tie point (i, j) -> (lon, lat) of that pixel's outer corner (PixelIsArea)
            let step = scale[0];
            let (lon_c, lat_c) = (tie[3] - tie[0] * step + step / 2.0, tie[4] + tie[1] * step - step / 2.0);
            let data: Vec<f32> = match dec.read_image()? {
                tiff::decoder::DecodingResult::F32(v) => v,
                tiff::decoder::DecodingResult::F64(v) => v.into_iter().map(|x| x as f32).collect(),
                tiff::decoder::DecodingResult::I16(v) => v.into_iter().map(|x| x as f32).collect(),
                tiff::decoder::DecodingResult::I32(v) => v.into_iter().map(|x| x as f32).collect(),
                _ => anyhow::bail!("{}: unsupported sample format", f.display()),
            };
            // the part of this tile inside the box (one pixel of margin)
            let c0 = (((lon_min - lon_c) / step).floor() as i64 - 1).max(0) as usize;
            let c1 = ((((lon_max - lon_c) / step).ceil() as i64 + 1).min(w as i64 - 1)).max(0) as usize;
            let r0 = (((lat_c - lat_max) / step).floor() as i64 - 1).max(0) as usize;
            let r1 = ((((lat_c - lat_min) / step).ceil() as i64 + 1).min(h as i64 - 1)).max(0) as usize;
            if c1 <= c0 || r1 <= r0 {
                continue;
            }
            let (cw, ch) = (c1 - c0 + 1, r1 - r0 + 1);
            let mut z = Vec::with_capacity(cw * ch);
            for r in r0..=r1 {
                z.extend_from_slice(&data[r * w as usize + c0..=r * w as usize + c1]);
            }
            let tile = Sea { lat0: lat_c - r0 as f64 * step, lon0: lon_c + c0 as f64 * step, step, w: cw, h: ch, z };
            eprintln!("bathymetry: {}: {cw}x{ch} cells of {:.4}° from {:.3}N {:.3}E", f.file_name().unwrap_or_default().to_string_lossy(), step, tile.lat0, tile.lon0);
            out = Some(match out {
                None => tile,
                Some(a) => a.merge(tile),
            });
        }
        Ok(out)
    }

    /// Union of two grids of the same resolution and alignment.
    fn merge(self, b: Sea) -> Sea {
        let s = self.step;
        let lat0 = self.lat0.max(b.lat0);
        let lon0 = self.lon0.min(b.lon0);
        let lat1 = (self.lat0 - (self.h - 1) as f64 * s).min(b.lat0 - (b.h - 1) as f64 * s);
        let lon1 = (self.lon0 + (self.w - 1) as f64 * s).max(b.lon0 + (b.w - 1) as f64 * s);
        let (w, h) = (((lon1 - lon0) / s).round() as usize + 1, ((lat0 - lat1) / s).round() as usize + 1);
        let mut z = vec![f32::NAN; w * h];
        for g in [&self, &b] {
            let (ox, oy) = (((g.lon0 - lon0) / s).round() as usize, ((lat0 - g.lat0) / s).round() as usize);
            for r in 0..g.h {
                for c in 0..g.w {
                    z[(oy + r) * w + ox + c] = g.z[r * g.w + c];
                }
            }
        }
        Sea { lat0, lon0, step: s, w, h, z }
    }

    /// Elevation (m) at a point, NaN outside the grid. Bicubic (Catmull–Rom)
    /// between the pixel centres, so the sea floor has no kinks along the
    /// 460 m cell edges.
    pub fn at(&self, lat: f64, lon: f64) -> f32 {
        let fc = (lon - self.lon0) / self.step;
        let fr = (self.lat0 - lat) / self.step;
        if fc < 0.0 || fr < 0.0 || fc > (self.w - 1) as f64 || fr > (self.h - 1) as f64 {
            return f32::NAN;
        }
        let (c0, r0) = ((fc.floor() as i64).min(self.w as i64 - 2), (fr.floor() as i64).min(self.h as i64 - 2));
        let (tc, tr) = ((fc - c0 as f64) as f32, (fr - r0 as f64) as f32);
        let v = |r: i64, c: i64| self.z[r.clamp(0, self.h as i64 - 1) as usize * self.w + c.clamp(0, self.w as i64 - 1) as usize];
        let cr = |p: [f32; 4], t: f32| -> f32 {
            let (a, b, c, d) = (p[0], p[1], p[2], p[3]);
            b + 0.5 * t * (c - a + t * (2.0 * a - 5.0 * b + 4.0 * c - d + t * (3.0 * (b - c) + d - a)))
        };
        let mut rows = [0f32; 4];
        for (k, r) in (r0 - 1..=r0 + 2).enumerate() {
            rows[k] = cr([v(r, c0 - 1), v(r, c0), v(r, c0 + 1), v(r, c0 + 2)], tc);
        }
        cr(rows, tr)
    }
}
