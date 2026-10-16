//! Ingestion of the additional vector/raster sources into tiled stores in
//! Minecraft coordinates: 基盤地図情報 基本項目 (roads, rails, buildings,
//! water structures), and later OSM, PLATEAU, vegetation, land cover.

use crate::gml::{self, KFeature};
use crate::proj::Lcc;
use crate::scan::{list_zips, read_entry};
use crate::vfeat::{self, VFeat};
use anyhow::Result;
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Margin (m) for assigning features to neighbouring tiles.
pub const TILE_MARGIN: f64 = 160.0;

/// 基本項目 layers we use and the class code of each <type> value.
pub const KIHON_LAYERS: &[(&str, &[&str])] = &[
    ("RdEdg", &["真幅道路", "庭園路等", "徒歩道", "トンネル内の道路", "建設中の道路"]),
    ("RdCompt", &["歩道", "分離帯", "側溝", "安全地帯"]),
    ("RailCL", &["普通鉄道", "トンネル内の鉄道", "特殊軌道", "索道", "路面の鉄道"]),
    ("BldA", &["普通建物", "堅ろう建物", "普通無壁舎", "堅ろう無壁舎"]),
    ("WStrA", &["ダム", "せき", "水門", "防波堤", "桟橋（鉄，コンクリート）", "敷石斜坂", "透過水制", "不透過水制", "砂防ダム", "滝"]),
    ("WStrL", &["ダム", "せき", "水門", "防波堤", "桟橋（鉄，コンクリート）", "敷石斜坂", "透過水制", "不透過水制", "砂防ダム", "滝"]),
];

pub fn kihon_class(layer: &str, ty: &str) -> u16 {
    for (l, types) in KIHON_LAYERS {
        if *l == layer {
            return types.iter().position(|t| *t == ty).map(|p| p as u16 + 1).unwrap_or(99);
        }
    }
    99
}

/// Newest `FG-GML-xxxxxx-ALL-yyyymmdd.zip` per 2nd mesh across all outer zips.
fn newest_all_packages(dir: &Path) -> Result<Vec<(String, PathBuf, String)>> {
    let mut best: HashMap<String, (String, PathBuf, String)> = HashMap::new();
    for z in list_zips(dir) {
        let f = std::fs::File::open(&z)?;
        let za = zip::ZipArchive::new(std::io::BufReader::new(f))?;
        for n in za.file_names() {
            let parts: Vec<&str> = n.trim_end_matches(".zip").split('-').collect();
            if parts.len() >= 5 && parts[3] == "ALL" {
                let e = best.entry(parts[2].to_string()).or_insert((String::new(), PathBuf::new(), String::new()));
                if parts[4] > e.0.as_str() {
                    *e = (parts[4].to_string(), z.clone(), n.to_string());
                }
            }
        }
    }
    let mut v: Vec<(String, PathBuf, String)> = best.into_iter().map(|(m, (_, o, i))| (m, o, i)).collect();
    v.sort();
    Ok(v)
}

/// Parse the 基本項目 layers of every 2nd mesh and write one tiled store per layer.
pub fn kihon(data: &Path, work: &Path) -> Result<()> {
    let t0 = std::time::Instant::now();
    let jobs = newest_all_packages(&data.join("基本情報"))?;
    eprintln!("kihon: {} 2nd-mesh packages", jobs.len());
    let lcc = Lcc::new();
    let acc: Mutex<BTreeMap<String, Vec<VFeat>>> = Mutex::new(BTreeMap::new());
    let types: Mutex<BTreeMap<(String, String), usize>> = Mutex::new(BTreeMap::new());
    jobs.par_iter().try_for_each(|(mesh, outer, inner)| -> Result<()> {
        let f = std::fs::File::open(outer)?;
        let mut za = zip::ZipArchive::new(std::io::BufReader::new(f))?;
        let bytes = read_entry(&mut za, inner)?;
        let mut iz = zip::ZipArchive::new(Cursor::new(&bytes[..]))?;
        let names: Vec<String> = iz.file_names().map(|s| s.to_string()).collect();
        let mut local: BTreeMap<String, Vec<VFeat>> = BTreeMap::new();
        let mut local_types: BTreeMap<(String, String), usize> = BTreeMap::new();
        for (layer, _) in KIHON_LAYERS {
            let pat = format!("-{layer}-");
            for n in names.iter().filter(|n| n.contains(&pat)) {
                let x = read_entry(&mut iz, n)?;
                for kf in gml::parse_kihon(&x, layer) {
                    *local_types.entry((layer.to_string(), kf.ty.clone())).or_default() += 1;
                    local.entry(layer.to_string()).or_default().push(to_vfeat(&lcc, layer, &kf, mesh));
                }
            }
        }
        let mut a = acc.lock().unwrap();
        for (k, v) in local {
            a.entry(k).or_default().extend(v);
        }
        let mut t = types.lock().unwrap();
        for (k, v) in local_types {
            *t.entry(k).or_default() += v;
        }
        Ok(())
    })?;
    eprintln!("kihon: parsed in {:.1}s", t0.elapsed().as_secs_f64());
    for ((l, ty), n) in types.into_inner().unwrap() {
        eprintln!("  {l} {ty}: {n} (class {})", kihon_class(&l, &ty));
    }
    for (layer, feats) in acc.into_inner().unwrap() {
        let p = work.join(format!("vec_kihon_{}.bin", layer.to_ascii_lowercase()));
        vfeat::write_tiled(&p, &feats, TILE_MARGIN)?;
        eprintln!("kihon: {} {} features -> {}", layer, feats.len(), p.display());
    }
    eprintln!("kihon: done in {:.1}s", t0.elapsed().as_secs_f64());
    Ok(())
}

// ---------------------------------------------------------------- WKB / GeoPackage

/// Parse a GeoPackage geometry blob (GP header + WKB) into polygons, each a
/// list of rings of (x, y) = (lon, lat).
pub fn gpkg_polygons(blob: &[u8]) -> Vec<Vec<Vec<(f64, f64)>>> {
    if blob.len() < 8 || &blob[..2] != b"GP" {
        return vec![];
    }
    let flags = blob[3];
    let env = match (flags >> 1) & 7 {
        0 => 0,
        1 => 32,
        2 | 3 => 48,
        4 => 64,
        _ => 0,
    };
    let mut p = 8 + env;
    let mut out = Vec::new();
    wkb(blob, &mut p, &mut out);
    out
}

fn wkb(d: &[u8], p: &mut usize, out: &mut Vec<Vec<Vec<(f64, f64)>>>) {
    if *p + 5 > d.len() {
        return;
    }
    let le = d[*p] == 1;
    *p += 1;
    let rd_u32 = |p: &mut usize| -> u32 {
        let b: [u8; 4] = d[*p..*p + 4].try_into().unwrap();
        *p += 4;
        if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }
    };
    let rd_f64 = |p: &mut usize| -> f64 {
        let b: [u8; 8] = d[*p..*p + 8].try_into().unwrap();
        *p += 8;
        if le { f64::from_le_bytes(b) } else { f64::from_be_bytes(b) }
    };
    let ty = rd_u32(p);
    let base = ty % 1000;
    let dims = match ty / 1000 {
        1 | 2 => 3,
        3 => 4,
        _ => 2,
    };
    match base {
        3 => {
            let nr = rd_u32(p) as usize;
            let mut poly = Vec::with_capacity(nr);
            for _ in 0..nr {
                let np = rd_u32(p) as usize;
                let mut ring = Vec::with_capacity(np);
                for _ in 0..np {
                    let x = rd_f64(p);
                    let y = rd_f64(p);
                    for _ in 2..dims {
                        rd_f64(p);
                    }
                    ring.push((x, y));
                }
                poly.push(ring);
            }
            out.push(poly);
        }
        6 => {
            let n = rd_u32(p) as usize;
            for _ in 0..n {
                wkb(d, p, out);
            }
        }
        _ => {}
    }
}

/// 環境省 1/25,000 現存植生図 (GeoPackage blocks covering the area) → tiled
/// polygons (nums[0] = 凡例コード, text = 凡例名) + legend list CSV.
pub fn vegmap(data: &Path, work: &Path, area: &crate::area::Area) -> Result<()> {
    let t0 = std::time::Instant::now();
    let dir = data.join("現存植生図2024");
    let lcc = Lcc::new();
    let mut feats: Vec<VFeat> = Vec::new();
    let mut legend: BTreeMap<String, (String, String, String, usize, f64)> = BTreeMap::new();
    for e in std::fs::read_dir(&dir)? {
        let p = e?.path();
        if !p.extension().is_some_and(|x| x == "gpkg") {
            continue;
        }
        let con = rusqlite::Connection::open_with_flags(&p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let table: String = con.query_row("select table_name from gpkg_contents where data_type='features'", [], |r| r.get(0))?;
        let q = format!(
            "select t.* from \"{table}\" t join \"rtree_{table}_geom\" r on t.fid = r.id \
             where r.maxx >= ?1 and r.minx <= ?2 and r.maxy >= ?3 and r.miny <= ?4"
        );
        let mut st = con.prepare(&q)?;
        let cols: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
        let ci = |name: &str| cols.iter().position(|c| c == name);
        let (Some(c_geom), Some(c_code), Some(c_name)) = (ci("geom"), ci("凡例コード"), ci("凡例名")) else {
            eprintln!("vegmap: {} lacks expected columns {:?}", p.display(), cols);
            continue;
        };
        let c_big = ci("植生区分");
        let c_nat = ci("植生自然度");
        let mut rows = st.query(rusqlite::params![area.lon_min - 0.01, area.lon_max + 0.01, area.lat_min - 0.01, area.lat_max + 0.01])?;
        let mut n = 0usize;
        while let Some(r) = rows.next()? {
            let blob: Vec<u8> = r.get(c_geom)?;
            let code: String = r.get::<_, Option<String>>(c_code)?.unwrap_or_default();
            let name: String = r.get::<_, Option<String>>(c_name)?.unwrap_or_default();
            let big: String = c_big.and_then(|c| r.get::<_, Option<String>>(c).ok().flatten()).unwrap_or_default();
            let nat: String = c_nat.and_then(|c| r.get::<_, Option<String>>(c).ok().flatten()).unwrap_or_default();
            for poly in gpkg_polygons(&blob) {
                let parts: Vec<Vec<[f64; 2]>> = poly
                    .iter()
                    .map(|ring| {
                        ring.iter()
                            .map(|&(lon, lat)| {
                                let (x, z) = lcc.ll_to_mc(lat, lon);
                                [x, z]
                            })
                            .collect()
                    })
                    .collect();
                // planar area of the exterior ring (m²) for the legend report
                let a = ring_area(&parts[0]).abs();
                let e = legend.entry(code.clone()).or_insert((name.clone(), big.clone(), nat.clone(), 0, 0.0));
                e.3 += 1;
                e.4 += a;
                feats.push(VFeat {
                    class: 0,
                    flags: 0,
                    nums: [code.parse::<f32>().unwrap_or(0.0), 0.0, 0.0, 0.0],
                    id: 0,
                    text: code.clone(),
                    area: true,
                    parts,
                    zs: vec![],
                });
            }
            n += 1;
        }
        eprintln!("vegmap: {} -> {} features", p.file_name().unwrap().to_string_lossy(), n);
    }
    vfeat::write_tiled(&work.join("vec_veg.bin"), &feats, TILE_MARGIN)?;
    let mut f = std::io::BufWriter::new(std::fs::File::create(work.join("veg_legend.csv"))?);
    use std::io::Write;
    writeln!(f, "凡例コード,凡例名,植生区分,植生自然度,ポリゴン数,面積km2")?;
    for (code, (name, big, nat, n, a)) in &legend {
        writeln!(f, "{code},{name},{big},{nat},{n},{:.3}", a / 1e6)?;
    }
    eprintln!("vegmap: {} polygons, {} legend codes in {:.1}s", feats.len(), legend.len(), t0.elapsed().as_secs_f64());
    Ok(())
}

pub fn ring_area(r: &[[f64; 2]]) -> f64 {
    let mut s = 0.0;
    for i in 0..r.len() {
        let [x0, z0] = r[i];
        let [x1, z1] = r[(i + 1) % r.len()];
        s += x0 * z1 - x1 * z0;
    }
    s / 2.0
}

fn to_vfeat(lcc: &Lcc, layer: &str, kf: &KFeature, mesh: &str) -> VFeat {
    let proj = |v: &Vec<(f64, f64)>| -> Vec<[f64; 2]> {
        v.iter()
            .map(|&(la, lo)| {
                let (x, z) = lcc.ll_to_mc(la, lo);
                [x, z]
            })
            .collect()
    };
    let area = !kf.rings.is_empty();
    let parts = if area { kf.rings.iter().map(proj).collect() } else { kf.lines.iter().map(proj).collect() };
    VFeat {
        class: kihon_class(layer, &kf.ty),
        flags: 0,
        nums: [0.0; 4],
        id: mesh.parse().unwrap_or(0),
        text: if kf.extra.is_empty() || kf.extra == "不明" { kf.ty.clone() } else { format!("{}|{}", kf.ty, kf.extra) },
        area,
        parts,
        zs: vec![],
    }
}
