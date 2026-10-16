//! Indexing of the downloaded GSI zip archives (zip-in-zip), version selection,
//! and building the 5m/10m caches and the water-polygon cache.

use crate::gml::{self, WaterKind, WaterPoly};
use crate::store::Lv;
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct DemEntry {
    pub lv: Lv,
    pub mesh: String,
    pub srs: String,
    pub fmdid: String,
    pub date: String,
    pub kind: String,
    pub outer: PathBuf,
    pub inner: String,
    pub xml: String,
}

fn lv_from_name(name: &str) -> Option<Lv> {
    let u = name.to_ascii_uppercase();
    if u.contains("-DEM1A-") {
        Some(Lv::M1)
    } else if u.contains("-DEM5A-") {
        Some(Lv::M5)
    } else if u.contains("-DEM10A-") {
        Some(Lv::M10)
    } else {
        None
    }
}

/// Read a whole entry of a zip archive.
pub fn read_entry<R: Read + std::io::Seek>(z: &mut zip::ZipArchive<R>, name: &str) -> Result<Vec<u8>> {
    let mut f = z.by_name(name)?;
    let mut v = Vec::with_capacity(f.size() as usize);
    f.read_to_end(&mut v)?;
    Ok(v)
}

fn read_prefix<R: Read + std::io::Seek>(z: &mut zip::ZipArchive<R>, idx: usize, n: usize) -> Result<(String, Vec<u8>)> {
    let f = z.by_index(idx)?;
    let name = f.name().to_string();
    let mut v = Vec::with_capacity(n);
    f.take(n as u64).read_to_end(&mut v)?;
    Ok((name, v))
}

pub fn list_zips(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip"))).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Index every DEM XML inside every outer zip of `dirs`.
pub fn index_dem(dirs: &[PathBuf]) -> Result<Vec<DemEntry>> {
    // (outer, inner) pairs
    let mut jobs = Vec::new();
    for d in dirs {
        for z in list_zips(d) {
            let f = std::fs::File::open(&z)?;
            let za = zip::ZipArchive::new(std::io::BufReader::new(f)).with_context(|| format!("{}", z.display()))?;
            for n in za.file_names() {
                if n.to_ascii_lowercase().ends_with(".zip") && lv_from_name(n).is_some() {
                    jobs.push((z.clone(), n.to_string()));
                }
            }
        }
    }
    eprintln!("index: {} inner archives", jobs.len());
    let out = Mutex::new(Vec::new());
    jobs.par_iter().try_for_each(|(outer, inner)| -> Result<()> {
        let f = std::fs::File::open(outer)?;
        let mut za = zip::ZipArchive::new(std::io::BufReader::new(f))?;
        let bytes = read_entry(&mut za, inner)?;
        let mut iz = zip::ZipArchive::new(Cursor::new(&bytes[..]))?;
        let mut local = Vec::new();
        for i in 0..iz.len() {
            let (name, head) = read_prefix(&mut iz, i, 4096)?;
            if !name.to_ascii_lowercase().ends_with(".xml") {
                continue;
            }
            let Some(lv) = lv_from_name(&name) else { continue };
            let m = gml::dem_meta(&head).with_context(|| format!("{} / {} / {}", outer.display(), inner, name))?;
            local.push(DemEntry {
                lv,
                mesh: m.mesh.clone(),
                srs: m.srs.clone(),
                fmdid: m.fmdid.clone(),
                date: m.date.clone(),
                kind: m.kind.clone(),
                outer: outer.clone(),
                inner: inner.clone(),
                xml: name,
            });
        }
        out.lock().unwrap().extend(local);
        Ok(())
    })?;
    let mut v = out.into_inner().unwrap();
    v.sort_by(|a, b| (a.lv as u8, &a.mesh, &a.date, &a.xml).cmp(&(b.lv as u8, &b.mesh, &b.date, &b.xml)));
    Ok(v)
}

pub fn is_jgd2024(srs: &str) -> bool {
    srs.to_ascii_lowercase().contains("jgd2024")
}

/// Pick one entry per (level, mesh): JGD2024 first, then newest data date,
/// then newest package (inner archive name).
pub fn select(entries: &[DemEntry]) -> Vec<DemEntry> {
    let mut groups: BTreeMap<(u8, String), Vec<&DemEntry>> = BTreeMap::new();
    for e in entries {
        groups.entry((e.lv as u8, e.mesh.clone())).or_default().push(e);
    }
    let mut out = Vec::new();
    for (_, g) in groups {
        let best = g
            .iter()
            .max_by(|a, b| (is_jgd2024(&a.srs), &a.date, &a.inner).cmp(&(is_jgd2024(&b.srs), &b.date, &b.inner)))
            .unwrap();
        out.push((*best).clone());
    }
    out
}

pub fn write_index(path: &Path, v: &[DemEntry]) -> Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(f, "level\tmesh\tsrs\tfmdid\tdate\tkind\touter\tinner\txml")?;
    for e in v {
        writeln!(
            f,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            e.lv.name(),
            e.mesh,
            e.srs,
            e.fmdid,
            e.date,
            e.kind,
            e.outer.display(),
            e.inner,
            e.xml
        )?;
    }
    Ok(())
}

/// Decode a set of selected entries (any level) and call `sink` with each grid.
/// Entries are grouped per inner archive so each inner zip is inflated once.
pub fn decode_entries(entries: &[&DemEntry], sink: &(dyn Fn(&DemEntry, Vec<gml::DemGrid>) + Sync)) -> Result<()> {
    let mut by_inner: BTreeMap<(PathBuf, String), Vec<&DemEntry>> = BTreeMap::new();
    for e in entries {
        by_inner.entry((e.outer.clone(), e.inner.clone())).or_default().push(e);
    }
    let groups: Vec<_> = by_inner.into_iter().collect();
    groups.par_iter().try_for_each(|((outer, inner), es)| -> Result<()> {
        let f = std::fs::File::open(outer)?;
        let mut za = zip::ZipArchive::new(std::io::BufReader::new(f))?;
        let bytes = read_entry(&mut za, inner)?;
        es.par_iter().try_for_each(|e| -> Result<()> {
            let mut iz = zip::ZipArchive::new(Cursor::new(&bytes[..]))?;
            let x = read_entry(&mut iz, &e.xml)?;
            let grids = gml::parse_dem(&x).with_context(|| format!("{} {}", e.inner, e.xml))?;
            sink(e, grids);
            Ok(())
        })
    })
}


// ---------------------------------------------------------------- water polygons

pub struct WaterIndex {
    pub polys: Vec<WaterPoly>,
    /// 2nd-mesh codes (6 digits) that have 基本情報 data at all
    pub meshes: BTreeSet<String>,
    pub type_counts: BTreeMap<String, usize>,
}

pub fn index_water(dir: &Path) -> Result<WaterIndex> {
    // newest package per 2nd mesh
    let mut best: HashMap<String, (String, PathBuf, String)> = HashMap::new();
    for z in list_zips(dir) {
        let f = std::fs::File::open(&z)?;
        let za = zip::ZipArchive::new(std::io::BufReader::new(f))?;
        for n in za.file_names() {
            // FG-GML-513760-ALL-20250701.zip
            let parts: Vec<&str> = n.trim_end_matches(".zip").split('-').collect();
            if parts.len() >= 5 && parts[3] == "ALL" {
                let mesh = parts[2].to_string();
                let date = parts[4].to_string();
                let e = best.entry(mesh).or_insert((String::new(), PathBuf::new(), String::new()));
                if date > e.0 {
                    *e = (date, z.clone(), n.to_string());
                }
            }
        }
    }
    let jobs: Vec<_> = best.into_iter().collect();
    let polys = Mutex::new(Vec::new());
    let types = Mutex::new(BTreeMap::new());
    jobs.par_iter().try_for_each(|(_, (_, outer, inner))| -> Result<()> {
        let f = std::fs::File::open(outer)?;
        let mut za = zip::ZipArchive::new(std::io::BufReader::new(f))?;
        let bytes = read_entry(&mut za, inner)?;
        let mut iz = zip::ZipArchive::new(Cursor::new(&bytes[..]))?;
        let names: Vec<String> = iz.file_names().filter(|n| n.contains("-WA-")).map(|s| s.to_string()).collect();
        let mut local_t = BTreeMap::new();
        let mut local = Vec::new();
        for n in names {
            let x = read_entry(&mut iz, &n)?;
            local.extend(gml::parse_wa(&x, &mut local_t));
        }
        polys.lock().unwrap().extend(local);
        let mut t = types.lock().unwrap();
        for (k, v) in local_t {
            *t.entry(k).or_insert(0) += v;
        }
        Ok(())
    })?;
    Ok(WaterIndex {
        polys: polys.into_inner().unwrap(),
        meshes: jobs.iter().map(|(m, _)| m.clone()).collect(),
        type_counts: types.into_inner().unwrap(),
    })
}

pub fn save_water(path: &Path, w: &WaterIndex) -> Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all(b"JPWATER1")?;
    f.write_all(&(w.meshes.len() as u32).to_le_bytes())?;
    for m in &w.meshes {
        f.write_all(&(m.len() as u32).to_le_bytes())?;
        f.write_all(m.as_bytes())?;
    }
    f.write_all(&(w.polys.len() as u32).to_le_bytes())?;
    for p in &w.polys {
        f.write_all(&[p.kind as u8])?;
        f.write_all(&(p.rings.len() as u32).to_le_bytes())?;
        for r in &p.rings {
            f.write_all(&(r.len() as u32).to_le_bytes())?;
            for (a, b) in r {
                f.write_all(&a.to_le_bytes())?;
                f.write_all(&b.to_le_bytes())?;
            }
        }
    }
    Ok(())
}

pub fn load_water(path: &Path) -> Result<WaterIndex> {
    let d = std::fs::read(path)?;
    let mut p = 8usize;
    let u32_ = |p: &mut usize| {
        let v = u32::from_le_bytes(d[*p..*p + 4].try_into().unwrap());
        *p += 4;
        v as usize
    };
    let n = u32_(&mut p);
    let mut meshes = BTreeSet::new();
    for _ in 0..n {
        let l = u32_(&mut p);
        meshes.insert(String::from_utf8_lossy(&d[p..p + l]).into_owned());
        p += l;
    }
    let n = u32_(&mut p);
    let mut polys = Vec::with_capacity(n);
    for _ in 0..n {
        let kind = if d[p] == 1 { WaterKind::Sea } else { WaterKind::Inland };
        p += 1;
        let nr = u32_(&mut p);
        let mut rings = Vec::with_capacity(nr);
        for _ in 0..nr {
            let np = u32_(&mut p);
            let mut r = Vec::with_capacity(np);
            for _ in 0..np {
                let a = f64::from_le_bytes(d[p..p + 8].try_into().unwrap());
                let b = f64::from_le_bytes(d[p + 8..p + 16].try_into().unwrap());
                p += 16;
                r.push((a, b));
            }
            rings.push(r);
        }
        polys.push(WaterPoly { kind, rings });
    }
    Ok(WaterIndex { polys, meshes, type_counts: BTreeMap::new() })
}
