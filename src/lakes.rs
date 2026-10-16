//! Global pass on the 5m lattice: land / sea / inland-water classification,
//! connected inland-water bodies, and their water-surface levels.
//!
//! Output: a 5m block store (`cache_waterlevel.bin`) holding the water level
//! at every inland-water cell and, extended, at cells within 2 cells of one
//! (NaN elsewhere). The 1m heightmap stage samples it for inland water.

use crate::area::Area;
use crate::gml::{self, WaterKind, is_elev};
use crate::raster;
use crate::scan::WaterIndex;
use crate::store::{LevelStore, Lv, ORIGIN_LAT, ORIGIN_LON, bkey, mesh_bounds};
use anyhow::Result;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;

pub const C_OUT: u8 = 0;
pub const C_LAND: u8 = 1;
pub const C_SEA: u8 = 2;
pub const C_INLAND: u8 = 3;
pub const C_NONE: u8 = 4;
const F_WA_SEA: u8 = 0x10;
const F_WA_INLAND: u8 = 0x20;
const F_VISITED: u8 = 0x80;

/// Sub-rectangle of the global 5m lattice covering the area.
#[derive(Clone, Copy, Debug)]
pub struct Lat5 {
    pub r0: i64,
    pub c0: i64,
    pub nr: usize,
    pub nc: usize,
}

impl Lat5 {
    pub fn from_area(a: &Area) -> Self {
        let pd = Lv::M5.per_deg();
        let r0 = ((ORIGIN_LAT - a.lat_max) * pd).round() as i64;
        let r1 = ((ORIGIN_LAT - a.lat_min) * pd).round() as i64;
        let c0 = ((a.lon_min - ORIGIN_LON) * pd).round() as i64;
        let c1 = ((a.lon_max - ORIGIN_LON) * pd).round() as i64;
        Lat5 { r0, c0, nr: (r1 - r0) as usize, nc: (c1 - c0) as usize }
    }
}

/// Set of 2nd meshes (global 2nd-mesh row/col on the 5m lattice: 1500 × 2250 cells)
pub fn mesh2_set(codes: &std::collections::BTreeSet<String>) -> HashSet<(i64, i64)> {
    let pd = Lv::M5.per_deg();
    codes
        .iter()
        .filter_map(|c| {
            let (lat, lon, dlat, _) = mesh_bounds(c)?;
            let r = ((ORIGIN_LAT - (lat + dlat)) * pd).round() as i64 / 1500;
            let k = ((lon - ORIGIN_LON) * pd).round() as i64 / 2250;
            Some((r, k))
        })
        .collect()
}

/// Rasterize WA polygons into flag bits of `cls` (nr × nc, lattice pixel space).
fn rasterize_wa(cls: &mut [u8], l: &Lat5, wa: &WaterIndex) {
    let pd = Lv::M5.per_deg();
    // polygons in lattice pixel coords
    let polys: Vec<(u8, Vec<Vec<(f64, f64)>>, f64, f64)> = wa
        .polys
        .par_iter()
        .map(|p| {
            let flag = if p.kind == WaterKind::Sea { F_WA_SEA } else { F_WA_INLAND };
            let rings: Vec<Vec<(f64, f64)>> = p
                .rings
                .iter()
                .map(|r| r.iter().map(|&(la, lo)| ((lo - ORIGIN_LON) * pd - l.c0 as f64, (ORIGIN_LAT - la) * pd - l.r0 as f64)).collect())
                .collect();
            let ymin = rings[0].iter().map(|p| p.1).fold(f64::MAX, f64::min);
            let ymax = rings[0].iter().map(|p| p.1).fold(f64::MIN, f64::max);
            (flag, rings, ymin, ymax)
        })
        .collect();
    const BAND: usize = 256;
    let nb = l.nr.div_ceil(BAND);
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); nb];
    for (i, p) in polys.iter().enumerate() {
        let b0 = (p.2.max(0.0) as usize / BAND).min(nb - 1);
        let b1 = ((p.3.max(0.0) as usize) / BAND).min(nb - 1);
        if p.3 < 0.0 || p.2 > l.nr as f64 {
            continue;
        }
        for b in b0..=b1 {
            buckets[b].push(i);
        }
    }
    cls.par_chunks_mut(BAND * l.nc).enumerate().for_each(|(b, band)| {
        let rows = band.len() / l.nc;
        let off = (b * BAND) as f64;
        for &i in &buckets[b] {
            let (flag, rings, _, _) = &polys[i];
            let shifted: Vec<Vec<(f64, f64)>> = rings.iter().map(|r| r.iter().map(|&(x, y)| (x, y - off)).collect()).collect();
            raster::fill_polygon(&shifted, l.nc, rows, |r, c0, c1| {
                for v in &mut band[r * l.nc + c0..r * l.nc + c1] {
                    *v |= *flag;
                }
            });
        }
    });
}

pub struct LakeOut {
    pub cls: Vec<u8>,
    pub lat: Lat5,
}

/// Classify the 5m lattice (no components yet).
pub fn classify5(area: &Area, m5: &LevelStore, wa: &WaterIndex) -> LakeOut {
    let l = Lat5::from_area(area);
    let mut cls = vec![0u8; l.nr * l.nc];
    rasterize_wa(&mut cls, &l, wa);
    let kihon = mesh2_set(&wa.meshes);
    let pd = Lv::M5.per_deg();
    let (bh, bw) = Lv::M5.block();
    cls.par_chunks_mut(l.nc).enumerate().for_each(|(r, row)| {
        let gr = l.r0 + r as i64;
        let lat = ORIGIN_LAT - (gr as f64 + 0.5) / pd;
        let mut cur_key = u64::MAX;
        let mut cur_blk: Option<&[f32]> = None;
        for (c, cell) in row.iter_mut().enumerate() {
            let gc = l.c0 + c as i64;
            let lon = ORIGIN_LON + (gc as f64 + 0.5) / pd;
            let flags = *cell;
            if !area.contains(lat, lon) {
                *cell = C_OUT;
                continue;
            }
            let k = bkey(gr / bh as i64, gc / bw as i64);
            if k != cur_key {
                cur_key = k;
                cur_blk = m5.block(k);
            }
            let v = match cur_blk {
                Some(b) => b[(gr % bh as i64) as usize * bw + (gc % bw as i64) as usize],
                None => f32::NAN,
            };
            // water: the WA polygons decide sea vs. inland; the DEM code only
            // where no polygon covers the cell
            *cell = if is_elev(v) {
                C_LAND
            } else if v == gml::SEA || v == gml::INLAND {
                if flags & F_WA_SEA != 0 {
                    C_SEA
                } else if flags & F_WA_INLAND != 0 || v == gml::INLAND {
                    C_INLAND
                } else {
                    C_SEA
                }
            } else if flags & F_WA_SEA != 0 {
                C_SEA
            } else if flags & F_WA_INLAND != 0 {
                C_INLAND
            } else if !kihon.contains(&(gr / 1500, gc / 2250)) {
                C_SEA
            } else {
                C_NONE
            };
        }
    });
    LakeOut { cls, lat: l }
}

struct Comp {
    cells: Vec<u32>,
    r0: usize,
    r1: usize,
    c0: usize,
    c1: usize,
}

pub struct CompResult {
    pub n: usize,
    pub n_samples: usize,
    pub lat: f64,
    pub lon: f64,
    pub p10: f32,
    pub p50: f32,
    pub p90: f32,
    pub mode: f32,
    pub flat_frac: f32,
    pub fmin: f32,
    pub fmax: f32,
}

/// Water level for every cell of a component.
///
/// Shore samples sit on inland cells that touch land (or sea, which pins the
/// level to 0 m). Where 1m data exists the sample is a low percentile of the
/// 1m land pixels bordering water around the cell (`shore1m`), which is much
/// closer to the true waterline than the 5m neighbour. The samples are
/// interpolated over the body (pull-push); the dominant level is then made
/// exactly flat at a low percentile of its own shore samples.
fn solve_comp(
    comp: &Comp,
    cls: &[u8],
    nc: usize,
    elev: &(dyn Fn(usize, usize) -> f32 + Sync),
    shore1m: &(dyn Fn(usize, usize) -> Option<f32> + Sync),
) -> (Vec<f32>, CompResult) {
    let (bw, bh) = (comp.c1 - comp.c0 + 1, comp.r1 - comp.r0 + 1);
    let mut grid = vec![f32::NAN; bw * bh];
    let mut samples = Vec::new();
    let mut sample_at: Vec<(usize, f32)> = Vec::new();
    let nr_total = cls.len() / nc;
    for (k, &i) in comp.cells.iter().enumerate() {
        let (r, c) = (i as usize / nc, i as usize % nc);
        let mut best = f32::INFINITY;
        let mut sea = false;
        let nb = [(r.wrapping_sub(1), c), (r + 1, c), (r, c.wrapping_sub(1)), (r, c + 1)];
        for (rr, cc) in nb {
            if rr >= nr_total || cc >= nc {
                continue;
            }
            match cls[rr * nc + cc] & 0x0f {
                C_LAND => best = best.min(elev(rr, cc)),
                C_SEA => sea = true,
                _ => {}
            }
        }
        if sea {
            best = 0.0;
        } else if best.is_finite() {
            if let Some(s) = shore1m(r, c) {
                best = s;
            }
        }
        if best.is_finite() {
            grid[(r - comp.r0) * bw + (c - comp.c0)] = best;
            samples.push(best);
            sample_at.push((k, best));
        }
    }
    let n_samples = samples.len();
    let (rc, cc) = ((comp.r0 + comp.r1) / 2, (comp.c0 + comp.c1) / 2);
    let mut res = CompResult {
        n: comp.cells.len(),
        n_samples,
        lat: rc as f64,
        lon: cc as f64,
        p10: f32::NAN,
        p50: f32::NAN,
        p90: f32::NAN,
        mode: f32::NAN,
        flat_frac: 0.0,
        fmin: f32::NAN,
        fmax: f32::NAN,
    };
    if samples.is_empty() {
        return (vec![f32::NAN; comp.cells.len()], res);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| samples[((samples.len() - 1) as f64 * p).round() as usize];
    res.p10 = pct(0.10);
    res.p50 = pct(0.50);
    res.p90 = pct(0.90);
    // interpolate inside the component
    raster::pull_push(&mut grid, bw, bh);
    let mut vals: Vec<f32> = comp.cells.iter().map(|&i| grid[(i as usize / nc - comp.r0) * bw + (i as usize % nc - comp.c0)]).collect();
    res.fmin = vals.iter().copied().fold(f32::INFINITY, f32::min);
    res.fmax = vals.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    // flatten the dominant level (lake surface) to one value
    if vals.len() >= 20 {
        let lo = res.fmin;
        let nbin = (((res.fmax - lo) / 0.1) as usize + 1).min(200_000);
        let mut hist = vec![0u32; nbin];
        for v in &vals {
            hist[(((v - lo) / 0.1) as usize).min(nbin - 1)] += 1;
        }
        // smooth over ±3 bins (±0.3 m) to find the densest level
        let mut best = (0u32, 0usize);
        let mut acc: u32 = hist[..7.min(nbin)].iter().sum();
        for b in 0..nbin {
            if acc > best.0 {
                best = (acc, b);
            }
            if b + 7 < nbin {
                acc += hist[b + 7];
            }
            acc -= hist[b];
        }
        let mode = lo + (best.1 as f32 + 3.5) * 0.1;
        let flat: Vec<bool> = vals.iter().map(|v| (*v - mode).abs() <= 0.6).collect();
        // flat level: 20th percentile of the shore samples of the flat part
        let mut fs: Vec<f32> = sample_at.iter().filter(|(k, _)| flat[*k]).map(|(_, s)| *s).collect();
        let level = if fs.len() >= 5 {
            fs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            fs[(fs.len() - 1) / 5].min(mode)
        } else {
            mode
        };
        let mut nflat = 0;
        for (v, f) in vals.iter_mut().zip(&flat) {
            if *f {
                *v = level;
                nflat += 1;
            }
        }
        res.mode = level;
        res.flat_frac = nflat as f32 / vals.len() as f32;
    }
    (vals, res)
}

pub fn run(area: &Area, work: &Path, wa: &WaterIndex) -> Result<()> {
    let t = std::time::Instant::now();
    let m5 = LevelStore::open(Lv::M5, &work.join("cache_5m.bin"))?;
    let LakeOut { mut cls, lat: l } = classify5(area, &m5, wa);
    eprintln!("lakes: classified {}x{} lattice in {:.1}s", l.nc, l.nr, t.elapsed().as_secs_f64());
    let mut counts = [0usize; 5];
    for v in &cls {
        counts[(*v & 0x0f) as usize] += 1;
    }
    eprintln!("lakes: cells out={} land={} sea={} inland={} none={}", counts[0], counts[1], counts[2], counts[3], counts[4]);
    let (bh, bw) = Lv::M5.block();
    let elev = |r: usize, c: usize| -> f32 {
        let gr = l.r0 + r as i64;
        let gc = l.c0 + c as i64;
        m5.block(bkey(gr / bh as i64, gc / bw as i64))
            .map(|b| b[(gr % bh as i64) as usize * bw + (gc % bw as i64) as usize])
            .unwrap_or(f32::NAN)
    };
    overview(work, &cls, &l, &elev)?;
    // connected components (4-neighbour) of inland water
    let t = std::time::Instant::now();
    let nc = l.nc;
    let mut comps: Vec<Comp> = Vec::new();
    let mut queue: Vec<u32> = Vec::new();
    for start in 0..cls.len() {
        if cls[start] & 0x0f != C_INLAND || cls[start] & F_VISITED != 0 {
            continue;
        }
        queue.clear();
        queue.push(start as u32);
        cls[start] |= F_VISITED;
        let mut head = 0;
        let (mut r0, mut r1, mut c0, mut c1) = (usize::MAX, 0, usize::MAX, 0);
        while head < queue.len() {
            let i = queue[head] as usize;
            head += 1;
            let (r, c) = (i / nc, i % nc);
            r0 = r0.min(r);
            r1 = r1.max(r);
            c0 = c0.min(c);
            c1 = c1.max(c);
            let push = |j: usize, q: &mut Vec<u32>, cls: &mut [u8]| {
                if cls[j] & 0x0f == C_INLAND && cls[j] & F_VISITED == 0 {
                    cls[j] |= F_VISITED;
                    q.push(j as u32);
                }
            };
            if r > 0 {
                push(i - nc, &mut queue, &mut cls);
            }
            if r + 1 < l.nr {
                push(i + nc, &mut queue, &mut cls);
            }
            if c > 0 {
                push(i - 1, &mut queue, &mut cls);
            }
            if c + 1 < nc {
                push(i + 1, &mut queue, &mut cls);
            }
        }
        comps.push(Comp { cells: queue.clone(), r0, r1, c0, c1 });
    }
    eprintln!("lakes: {} inland water bodies in {:.1}s", comps.len(), t.elapsed().as_secs_f64());
    let t = std::time::Instant::now();
    // largest first so the big ones start early
    comps.sort_by_key(|c| std::cmp::Reverse(c.cells.len()));
    let m1 = LevelStore::open(Lv::M1, &work.join("cache_1m.bin"))?;
    let (b1h, b1w) = Lv::M1.block();
    let cls_ref = &cls;
    // low percentile of 1m land pixels that border water, in the 15 m × 15 m
    // window around the 5m cell (r, c) (5m cell = 5 × 5 1m cells, same origin)
    let shore1m = |r: usize, c: usize| -> Option<f32> {
        let gr1 = (l.r0 + r as i64) * 5;
        let gc1 = (l.c0 + c as i64) * 5;
        let mut cache: [(u64, Option<&[f32]>); 4] = [(u64::MAX, None); 4];
        let mut get = |rr: i64, cc: i64| -> f32 {
            let k = bkey(rr / b1h as i64, cc / b1w as i64);
            let mut blk = None;
            let mut found = false;
            for e in cache.iter() {
                if e.0 == k {
                    blk = e.1;
                    found = true;
                    break;
                }
            }
            if !found {
                blk = m1.block(k);
                cache.rotate_right(1);
                cache[0] = (k, blk);
            }
            match blk {
                Some(b) => b[(rr % b1h as i64) as usize * b1w + (cc % b1w as i64) as usize],
                None => f32::NAN,
            }
        };
        let is_water = |v: f32, rr: i64, cc: i64| -> bool {
            if v == gml::INLAND || v == gml::SEA {
                return true;
            }
            if v.is_nan() {
                // no 1m value: trust the 5m class underneath
                let (r5, c5) = (rr / 5 - l.r0, cc / 5 - l.c0);
                if r5 >= 0 && c5 >= 0 && (r5 as usize) < l.nr && (c5 as usize) < l.nc {
                    let k = cls_ref[r5 as usize * l.nc + c5 as usize] & 0x0f;
                    return k == C_INLAND || k == C_SEA;
                }
            }
            false
        };
        let mut v: [f32; 225] = [0.0; 225];
        let mut n = 0;
        for rr in gr1 - 5..gr1 + 10 {
            for cc in gc1 - 5..gc1 + 10 {
                let h = get(rr, cc);
                if !is_elev(h) {
                    continue;
                }
                let nb = [(rr - 1, cc), (rr + 1, cc), (rr, cc - 1), (rr, cc + 1)];
                if nb.iter().any(|&(a, b)| is_water(get(a, b), a, b)) {
                    v[n] = h;
                    n += 1;
                }
            }
        }
        if n == 0 {
            return None;
        }
        let s = &mut v[..n];
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        Some(s[(n - 1) / 5])
    };
    let solved: Vec<(Vec<f32>, CompResult)> = comps.par_iter().map(|c| solve_comp(c, &cls, nc, &elev, &shore1m)).collect();
    eprintln!("lakes: levels solved in {:.1}s", t.elapsed().as_secs_f64());
    // level at inland cells, extended 2 cells outwards
    let mut nan_comps = 0;
    let mut cell_vals: Vec<(u32, f32)> = Vec::new();
    for (comp, (vals, _)) in comps.iter().zip(&solved) {
        if vals.first().is_some_and(|v| v.is_nan()) {
            nan_comps += 1;
            continue;
        }
        cell_vals.extend(comp.cells.iter().copied().zip(vals.iter().copied()));
    }
    let nblk = save_extended(&cell_vals, &l, &work.join("cache_waterlevel.bin"))?;
    drop(cell_vals);
    eprintln!("lakes: {} bodies without shore samples; water-level cache {} blocks", nan_comps, nblk);
    holes(work, &mut cls, &l, &elev)?;
    // report
    let pd = Lv::M5.per_deg();
    let mut f = std::io::BufWriter::new(std::fs::File::create(work.join("lakes.csv"))?);
    writeln!(f, "id,cells,area_km2,lat,lon,shore_samples,p10,p50,p90,level_mode,flat_fraction,level_min,level_max")?;
    for (i, (_, r)) in solved.iter().enumerate() {
        if r.n < 400 {
            continue;
        }
        let lat = ORIGIN_LAT - (l.r0 as f64 + r.lat + 0.5) / pd;
        let lon = ORIGIN_LON + (l.c0 as f64 + r.lon + 0.5) / pd;
        writeln!(
            f,
            "{},{},{:.4},{:.5},{:.5},{},{:.2},{:.2},{:.2},{:.2},{:.3},{:.2},{:.2}",
            i,
            r.n,
            r.n as f64 * 6.17 * 5.05 / 1e6, // 5m cell ≈ 6.17 m (N-S) × 5.05 m (E-W) at 35°N
            lat,
            lon,
            r.n_samples,
            r.p10,
            r.p50,
            r.p90,
            r.mode,
            r.flat_frac,
            r.fmin,
            r.fmax
        )?;
    }
    Ok(())
}

/// Write per-cell values (local lattice index, value) into a sparse 5m block
/// store, extending each value 2 cells outwards (mean of values within ±2
/// cells) so the 1m stage can interpolate up to the edges. Returns #blocks.
fn save_extended(cell_vals: &[(u32, f32)], l: &Lat5, path: &Path) -> Result<usize> {
    let (bh, bw) = Lv::M5.block();
    let nc = l.nc;
    let mut vblocks: HashMap<u64, Vec<f32>> = HashMap::new();
    let mut last: (u64, *mut f32) = (u64::MAX, std::ptr::null_mut());
    for &(i, v) in cell_vals {
        let gr = l.r0 + (i as usize / nc) as i64;
        let gc = l.c0 + (i as usize % nc) as i64;
        let k = bkey(gr / bh as i64, gc / bw as i64);
        if k != last.0 {
            last = (k, vblocks.entry(k).or_insert_with(|| vec![f32::NAN; bh * bw]).as_mut_ptr());
        }
        unsafe { *last.1.add((gr % bh as i64) as usize * bw + (gc % bw as i64) as usize) = v };
    }
    let mut keys: HashSet<u64> = HashSet::new();
    for &k in vblocks.keys() {
        let (br, bc) = ((k >> 32) as i64, (k & 0xffff_ffff) as u32 as i64);
        for dr in -1..=1 {
            for dc in -1..=1 {
                keys.insert(bkey(br + dr, bc + dc));
            }
        }
    }
    let keys: Vec<u64> = keys.into_iter().collect();
    let blocks: HashMap<u64, Vec<f32>> = keys
        .par_iter()
        .filter_map(|&k| {
            let (br, bc) = ((k >> 32) as i64, (k & 0xffff_ffff) as u32 as i64);
            let (pw, ph) = (bw + 4, bh + 4);
            let mut pad = vec![f32::NAN; pw * ph];
            for dr in -1i64..=1 {
                for dc in -1i64..=1 {
                    let Some(b) = vblocks.get(&bkey(br + dr, bc + dc)) else { continue };
                    for r in 0..bh as i64 {
                        let pr = r + dr * bh as i64 + 2;
                        if pr < 0 || pr >= ph as i64 {
                            continue;
                        }
                        for c in 0..bw as i64 {
                            let pc = c + dc * bw as i64 + 2;
                            if pc < 0 || pc >= pw as i64 {
                                continue;
                            }
                            pad[pr as usize * pw + pc as usize] = b[(r * bw as i64 + c) as usize];
                        }
                    }
                }
            }
            let mut out = vec![f32::NAN; bh * bw];
            let mut any = false;
            for r in 0..bh {
                for c in 0..bw {
                    let own = pad[(r + 2) * pw + c + 2];
                    if !own.is_nan() {
                        out[r * bw + c] = own;
                        any = true;
                        continue;
                    }
                    let (mut s, mut n) = (0f32, 0u32);
                    for dr in 0..5 {
                        for dc in 0..5 {
                            let v = pad[(r + dr) * pw + c + dc];
                            if !v.is_nan() {
                                s += v;
                                n += 1;
                            }
                        }
                    }
                    if n > 0 {
                        out[r * bw + c] = s / n as f32;
                        any = true;
                    }
                }
            }
            any.then_some((k, out))
        })
        .collect();
    crate::store::save_blocks(Lv::M5, &blocks, path)?;
    Ok(blocks.len())
}

/// Fill land cells without any DEM data ("holes") by pull-push interpolation
/// from the surrounding land (sea counts as 0 m), per connected hole, and
/// store the result as `cache_holefill.bin`. Also writes `holes.csv`.
fn holes(work: &Path, cls: &mut [u8], l: &Lat5, elev: &(dyn Fn(usize, usize) -> f32 + Sync)) -> Result<()> {
    let t = std::time::Instant::now();
    let nc = l.nc;
    let nr = l.nr;
    let mut comps: Vec<Comp> = Vec::new();
    let mut queue: Vec<u32> = Vec::new();
    for start in 0..cls.len() {
        if cls[start] & 0x0f != C_NONE || cls[start] & F_VISITED != 0 {
            continue;
        }
        queue.clear();
        queue.push(start as u32);
        cls[start] |= F_VISITED;
        let mut head = 0;
        let (mut r0, mut r1, mut c0, mut c1) = (usize::MAX, 0, usize::MAX, 0);
        while head < queue.len() {
            let i = queue[head] as usize;
            head += 1;
            let (r, c) = (i / nc, i % nc);
            r0 = r0.min(r);
            r1 = r1.max(r);
            c0 = c0.min(c);
            c1 = c1.max(c);
            let nbs = [
                (r > 0).then(|| i - nc),
                (r + 1 < nr).then(|| i + nc),
                (c > 0).then(|| i - 1),
                (c + 1 < nc).then(|| i + 1),
            ];
            for j in nbs.into_iter().flatten() {
                if cls[j] & 0x0f == C_NONE && cls[j] & F_VISITED == 0 {
                    cls[j] |= F_VISITED;
                    queue.push(j as u32);
                }
            }
        }
        comps.push(Comp { cells: queue.clone(), r0, r1, c0, c1 });
    }
    comps.sort_by_key(|c| std::cmp::Reverse(c.cells.len()));
    const M: usize = 8;
    let cls_ref: &[u8] = cls;
    let filled: Vec<Vec<f32>> = comps
        .par_iter()
        .map(|comp| {
            let (r0, c0) = (comp.r0.saturating_sub(M), comp.c0.saturating_sub(M));
            let (r1, c1) = ((comp.r1 + M).min(nr - 1), (comp.c1 + M).min(nc - 1));
            let (gw, gh) = (c1 - c0 + 1, r1 - r0 + 1);
            let mut grid = vec![f32::NAN; gw * gh];
            for r in r0..=r1 {
                for c in c0..=c1 {
                    grid[(r - r0) * gw + (c - c0)] = match cls_ref[r * nc + c] & 0x0f {
                        C_LAND => elev(r, c),
                        C_SEA => 0.0,
                        _ => f32::NAN,
                    };
                }
            }
            raster::pull_push(&mut grid, gw, gh);
            comp.cells.iter().map(|&i| grid[(i as usize / nc - r0) * gw + (i as usize % nc - c0)]).collect()
        })
        .collect();
    let mut cell_vals: Vec<(u32, f32)> = Vec::new();
    for (comp, vals) in comps.iter().zip(&filled) {
        for (&i, &v) in comp.cells.iter().zip(vals) {
            if !v.is_nan() {
                cell_vals.push((i, v));
            }
        }
    }
    let nblk = save_extended(&cell_vals, l, &work.join("cache_holefill.bin"))?;
    // report (cell area ≈ 6.17 m × 5.05 m at 35°N)
    let pd = Lv::M5.per_deg();
    let total: usize = comps.iter().map(|c| c.cells.len()).sum();
    let cell_m2 = 6.17 * 5.05;
    let mut f = std::io::BufWriter::new(std::fs::File::create(work.join("holes.csv"))?);
    writeln!(f, "id,cells,area_km2,lat_center,lon_center,extent_km_ns,extent_km_ew")?;
    for (i, c) in comps.iter().enumerate() {
        if c.cells.len() < 2000 {
            break;
        }
        let lat = ORIGIN_LAT - (l.r0 as f64 + (c.r0 + c.r1) as f64 / 2.0 + 0.5) / pd;
        let lon = ORIGIN_LON + (l.c0 as f64 + (c.c0 + c.c1) as f64 / 2.0 + 0.5) / pd;
        writeln!(
            f,
            "{},{},{:.3},{:.5},{:.5},{:.2},{:.2}",
            i,
            c.cells.len(),
            c.cells.len() as f64 * cell_m2 / 1e6,
            lat,
            lon,
            (c.r1 - c.r0 + 1) as f64 * 6.17 / 1000.0,
            (c.c1 - c.c0 + 1) as f64 * 5.05 / 1000.0
        )?;
    }
    eprintln!(
        "holes: {} gaps, {} cells (~{:.1} km²), largest {} cells; fill cache {} blocks in {:.1}s",
        comps.len(),
        total,
        total as f64 * cell_m2 / 1e6,
        comps.first().map(|c| c.cells.len()).unwrap_or(0),
        nblk,
        t.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Diagnostic PNG of the 5m classification (1 px = 16 cells = 80 m).
fn overview(work: &Path, cls: &[u8], l: &Lat5, elev: &(dyn Fn(usize, usize) -> f32 + Sync)) -> Result<()> {
    const S: usize = 16;
    let (w, h) = (l.nc / S, l.nr / S);
    let mut rgb = vec![0u8; w * h * 3];
    rgb.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            // any hole in the block wins (so small gaps stay visible)
            let mut hole = false;
            for dy in 0..S {
                for dx in 0..S {
                    if cls[(y * S + dy) * l.nc + x * S + dx] & 0x0f == C_NONE {
                        hole = true;
                    }
                }
            }
            let (r, c) = (y * S + S / 2, x * S + S / 2);
            let k = cls[r * l.nc + c] & 0x0f;
            let px = if hole {
                [255, 0, 0]
            } else {
                match k {
                    C_OUT => [0, 0, 0],
                    C_SEA => [20, 40, 110],
                    C_INLAND => [0, 200, 255],
                    C_LAND => {
                        let e = elev(r, c).max(0.0);
                        let t = (e / 3800.0).sqrt().min(1.0);
                        [(60.0 + 180.0 * t) as u8, (140.0 + 80.0 * t) as u8, (60.0 + 180.0 * t) as u8]
                    }
                    _ => [255, 0, 0],
                }
            };
            row[x * 3..x * 3 + 3].copy_from_slice(&px);
        }
    });
    raster::write_png(&work.join("overview_5m_classes.png"), w, h, &rgb)
}
