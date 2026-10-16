mod area;
mod features;
mod rgen;
mod roads;
mod gml;
mod heightmap;
mod ingest;
mod lakes;
mod landcover;
mod lulc;
mod osm;
mod osmfind;
mod network;
mod plateau;
mod bathy;
mod blocks;
mod chunk;
mod infra;
mod raster;
mod nbt;
mod proj;
mod region;
mod scan;
mod store;
mod tiff;
mod trees;
mod verify;
mod view;
mod vfeat;
mod world;

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::time::Instant;
use store::Lv;

// The Windows system allocator serializes the many short-lived chunk buffers
// across threads; mimalloc keeps the world writer CPU-bound.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub const DEFAULT_DATA: &str = r"E:\data";
pub const DEFAULT_WORK: &str = r"E:\data\mc1m_work";
pub const DEFAULT_WORLD: &str = r"C:\mc1m_world";
pub const DEFAULT_HEIGHTMAP: &str = r"E:\data\heightmap";
pub const DEFAULT_PLATEAU: &str = r"E:\data\PLATEAU";
pub const DEFAULT_FEATURES: &str = r"E:\data\features";
pub const DEFAULT_LCMAP: &str = concat!(env!("CARGO_MANIFEST_DIR"), r"\config\landcover_map.csv");

/// Minimal `--key value` option parsing.
pub struct Opts {
    pub pos: Vec<String>,
    pub kv: BTreeMap<String, String>,
}

impl Opts {
    fn parse(args: &[String]) -> Self {
        let mut pos = Vec::new();
        let mut kv = BTreeMap::new();
        let mut i = 0;
        while i < args.len() {
            if let Some(k) = args[i].strip_prefix("--") {
                // `--flag` without a value (next token missing or another option)
                match args.get(i + 1) {
                    Some(v) if !v.starts_with("--") => {
                        kv.insert(k.to_string(), v.clone());
                        i += 2;
                    }
                    _ => {
                        kv.insert(k.to_string(), String::new());
                        i += 1;
                    }
                }
            } else {
                pos.push(args[i].clone());
                i += 1;
            }
        }
        Opts { pos, kv }
    }
    pub fn path(&self, k: &str, def: &str) -> PathBuf {
        PathBuf::from(self.kv.get(k).map(|s| s.as_str()).unwrap_or(def))
    }
    pub fn get<T: std::str::FromStr>(&self, k: &str, def: T) -> T {
        self.kv.get(k).and_then(|s| s.parse().ok()).unwrap_or(def)
    }
}

/// Target area: `--meshes` if given (and remembered in <work>/area.txt),
/// else the remembered list, else the default 9 meshes.
fn area_of(o: &Opts, work: &std::path::Path) -> Result<area::Area> {
    let f = work.join("area.txt");
    let list = match o.kv.get("meshes") {
        Some(m) => {
            std::fs::create_dir_all(work)?;
            std::fs::write(&f, m)?;
            m.clone()
        }
        None => std::fs::read_to_string(&f).map(|s| s.trim().to_string()).unwrap_or_else(|_| area::DEFAULT_MESHES.to_string()),
    };
    let a = area::Area::new(&list)?;
    eprintln!("area: 1st meshes {}", a.meshes.join(","));
    Ok(a)
}

fn read_maybe_gz(p: &PathBuf) -> Result<Vec<u8>> {
    let d = std::fs::read(p).with_context(|| format!("{}", p.display()))?;
    if d.len() > 2 && d[0] == 0x1f && d[1] == 0x8b {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&d[..]).read_to_end(&mut out)?;
        Ok(out)
    } else {
        Ok(d)
    }
}

fn cmd_scan(o: &Opts) -> Result<()> {
    let data = o.path("data", DEFAULT_DATA);
    let work = o.path("work", DEFAULT_WORK);
    let idx = work.join("index");
    std::fs::create_dir_all(&idx)?;
    let t = Instant::now();
    let dirs = vec![data.join("DEM1A"), data.join("DEM5A"), data.join("DEM10A")];
    let all = scan::index_dem(&dirs)?;
    scan::write_index(&idx.join("dem_all.tsv"), &all)?;
    let sel = scan::select(&all);
    scan::write_index(&idx.join("dem_selected.tsv"), &sel)?;
    eprintln!("indexed {} DEM xml ({} selected) in {:.1}s", all.len(), sel.len(), t.elapsed().as_secs_f64());
    for lv in [Lv::M1, Lv::M5, Lv::M10] {
        let a: Vec<_> = all.iter().filter(|e| e.lv == lv).collect();
        let s: Vec<_> = sel.iter().filter(|e| e.lv == lv).collect();
        let mut srs: BTreeMap<String, usize> = BTreeMap::new();
        for e in &s {
            *srs.entry(format!("{} {}", e.srs, e.fmdid)).or_default() += 1;
        }
        let mut srs_all: BTreeMap<String, usize> = BTreeMap::new();
        for e in &a {
            *srs_all.entry(format!("{} {}", e.srs, e.fmdid)).or_default() += 1;
        }
        eprintln!("{}: {} files, {} unique meshes; selected srs {:?}; all srs {:?}", lv.name(), a.len(), s.len(), srs, srs_all);
    }
    if o.kv.contains_key("index-only") {
        return Ok(());
    }
    let levels: Vec<Lv> = match o.kv.get("levels").map(|s| s.as_str()) {
        Some(s) => s
            .split(',')
            .map(|x| match x {
                "1m" => Lv::M1,
                "5m" => Lv::M5,
                _ => Lv::M10,
            })
            .collect(),
        None => vec![Lv::M10, Lv::M5, Lv::M1],
    };
    for lv in levels {
        let t = Instant::now();
        let n = store::build_cache(lv, &sel, &work.join(format!("cache_{}.bin", lv.name())))?;
        eprintln!("{} cache: {} blocks in {:.1}s", lv.name(), n, t.elapsed().as_secs_f64());
    }
    if o.kv.contains_key("no-water") {
        return Ok(());
    }
    let t = Instant::now();
    let w = scan::index_water(&data.join("基本情報"))?;
    scan::save_water(&work.join("water_polys.bin"), &w)?;
    let npts: usize = w.polys.iter().map(|p| p.rings.iter().map(|r| r.len()).sum::<usize>()).sum();
    eprintln!(
        "water: {} polygons ({} pts) from {} meshes in {:.1}s; WA types {:?}",
        w.polys.len(),
        npts,
        w.meshes.len(),
        t.elapsed().as_secs_f64(),
        w.type_counts
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let o = Opts::parse(&args[2.min(args.len())..]);
    match cmd {
        "scan" => cmd_scan(&o)?,
        "plateau" => plateau::run(&o.path("plateau", DEFAULT_PLATEAU), &o.path("work", DEFAULT_WORK))?,
        "lcmap" => {
            let work = o.path("work", DEFAULT_WORK);
            let legend = landcover::read_veg_legend(&work.join("veg_legend.csv"))?;
            let p = o.path("lcmap", DEFAULT_LCMAP);
            let m = landcover::load_or_create(&p, &legend)?;
            eprintln!("landcover map: {} classes -> {}", m.len(), p.display());
        }
        "features" => {
            let work = o.path("work", DEFAULT_WORK);
            let hm = o.path("heightmap", DEFAULT_HEIGHTMAP);
            let area = area_of(&o, &work)?;
            let legend = landcover::read_veg_legend(&work.join("veg_legend.csv"))?;
            let lcmap = landcover::classes(&o.path("lcmap", DEFAULT_LCMAP), &legend)?;
            let ctx = features::Ctx::new(&work, &hm, &o.path("out", DEFAULT_FEATURES), &o.path("data", DEFAULT_DATA), &lcmap, &area)?;
            let mut tiles: Vec<(i64, i64)> = Vec::new();
            if let Some(s) = o.kv.get("tiles") {
                for p in s.split(';') {
                    let v: Vec<i64> = p.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    if v.len() == 2 {
                        tiles.push((v[0].div_euclid(heightmap::TILE), v[1].div_euclid(heightmap::TILE)));
                    }
                }
            } else {
                for e in std::fs::read_dir(&hm)? {
                    let n = e?.file_name().to_string_lossy().to_string();
                    if let Some(r) = n.strip_prefix("height_X").and_then(|s| s.strip_suffix(".tif")) {
                        let (xs, zs) = r.split_once("_Z").unwrap();
                        tiles.push((xs.parse::<i64>()? / heightmap::TILE, zs.parse::<i64>()? / heightmap::TILE));
                    }
                }
                tiles.sort_by_key(|t| (t.1, t.0));
            }
            let mut seen = std::collections::HashSet::new();
            tiles.retain(|t| seen.insert(*t));
            features::run(&ctx, &tiles, o.get("concurrency", 3usize))?;
        }
        "kihon" => ingest::kihon(&o.path("data", DEFAULT_DATA), &o.path("work", DEFAULT_WORK))?,
        "osm" => {
            // osm [PBF ...] : roads, railways, platforms and land use from OSM extracts
            let work = o.path("work", DEFAULT_WORK);
            let area = area_of(&o, &work)?;
            let pbfs: Vec<PathBuf> = if o.pos.is_empty() {
                let dir = o.path("data", DEFAULT_DATA).join("OSM");
                let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)?
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.to_string_lossy().ends_with(".osm.pbf"))
                    .collect();
                v.sort();
                v
            } else {
                o.pos.iter().map(PathBuf::from).collect()
            };
            osm::run(&pbfs, &work, &area)?;
        }
        "trees" => {
            // trees [--forest DIR] [--las DIR] : Shizuoka per-tree points and LiDAR canopy heights
            let data = o.path("data", DEFAULT_DATA);
            let work = o.path("work", DEFAULT_WORK);
            if !o.kv.contains_key("no-forest") {
                trees::ingest_forest(&o.kv.get("forest").map(PathBuf::from).unwrap_or_else(|| data.join("森林資源解析")), &work)?;
            }
            if !o.kv.contains_key("no-las") {
                trees::ingest_las(&o.kv.get("las").map(PathBuf::from).unwrap_or_else(|| data.join("VIRTUAL SHIZUOKA").join("LP_Original")), &work)?;
            }
        }
        "osmfind" => {
            // osmfind --names "名前,名前,..." [PBF ...] : positions of named OSM features
            let names: Vec<String> = o.kv.get("names").map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default();
            let pbfs: Vec<PathBuf> = if o.pos.is_empty() {
                let dir = o.path("data", DEFAULT_DATA).join("OSM");
                let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.to_string_lossy().ends_with(".osm.pbf")).collect();
                v.sort();
                v
            } else {
                o.pos.iter().map(PathBuf::from).collect()
            };
            osmfind::run(&pbfs, &names)?;
        }
        "network" => network::run(&o.path("work", DEFAULT_WORK), &o.path("heightmap", DEFAULT_HEIGHTMAP))?,
        "vegmap" => {
            let work = o.path("work", DEFAULT_WORK);
            let area = area_of(&o, &work)?;
            ingest::vegmap(&o.path("data", DEFAULT_DATA), &work, &area)?
        }
        "lakes" => {
            let work = o.path("work", DEFAULT_WORK);
            let area = area_of(&o, &work)?;
            let t = Instant::now();
            let wa = scan::load_water(&work.join("water_polys.bin"))?;
            eprintln!("loaded {} water polygons in {:.1}s", wa.polys.len(), t.elapsed().as_secs_f64());
            lakes::run(&area, &work, &wa)?;
            eprintln!("lakes done in {:.1}s", t.elapsed().as_secs_f64());
        }
        "nbtdump" => {
            let d = read_maybe_gz(&PathBuf::from(&o.pos[0]))?;
            let (_, t) = nbt::read_root(&d)?;
            let mut s = String::new();
            nbt::dump(&t, 0, &mut s);
            print!("{s}");
        }
        "chunkdump" => {
            let lx: usize = o.pos[1].parse()?;
            let lz: usize = o.pos[2].parse()?;
            let Some(d) = region::read_chunk(&PathBuf::from(&o.pos[0]), lx, lz)? else {
                bail!("chunk absent");
            };
            let (_, t) = nbt::read_root(&d)?;
            let mut s = String::new();
            nbt::dump(&t, 0, &mut s);
            print!("{s}");
        }
        "heightmap" => {
            let work = o.path("work", DEFAULT_WORK);
            let out = o.path("out", DEFAULT_HEIGHTMAP);
            let area = area_of(&o, &work)?;
            let t = Instant::now();
            let wa = scan::load_water(&work.join("water_polys.bin"))?;
            let ctx = heightmap::Ctx::new(area, &work, wa)?;
            eprintln!("heightmap: context ready in {:.1}s", t.elapsed().as_secs_f64());
            // --tiles "X,Z;X,Z" restricts to tiles containing those Minecraft coordinates
            let only = o.kv.get("tiles").map(|s| {
                s.split(';')
                    .filter_map(|p| {
                        let v: Vec<i64> = p.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                        (v.len() == 2).then(|| (v[0].div_euclid(heightmap::TILE), v[1].div_euclid(heightmap::TILE)))
                    })
                    .collect::<Vec<_>>()
            });
            heightmap::run(&ctx, &out, only, o.kv.contains_key("force"), o.get("concurrency", 4usize))?;
            eprintln!("heightmap done in {:.1}s", t.elapsed().as_secs_f64());
        }
        "world" => {
            let hm = o.path("heightmap", DEFAULT_HEIGHTMAP);
            let world_dir = o.path("world", &format!(r"{DEFAULT_WORLD}\japan_dem_1m"));
            let name = o.kv.get("name").cloned().unwrap_or_else(|| "日本 1m DEM（静岡・山梨とその周辺）".to_string());
            // --regions "rx,rz;rx,rz" or --around "X,Z,R" (regions within R blocks of X,Z) for partial builds
            let mut only: Option<Vec<(i64, i64)>> = o.kv.get("regions").map(|s| {
                s.split(';')
                    .filter_map(|p| {
                        let v: Vec<i64> = p.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                        (v.len() == 2).then(|| (v[0], v[1]))
                    })
                    .collect()
            });
            if let Some(a) = o.kv.get("around") {
                let v: Vec<i64> = a.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                let (x, z, r) = (v[0], v[1], v[2]);
                let mut l = Vec::new();
                for rz in (z - r).div_euclid(512)..=(z + r).div_euclid(512) {
                    for rx in (x - r).div_euclid(512)..=(x + r).div_euclid(512) {
                        l.push((rx, rz));
                    }
                }
                only = Some(l);
            }
            let ro = world::RunOpts { level: o.get("level", 6u32), only_regions: only, force: o.kv.contains_key("force") };
            let work = o.path("work", DEFAULT_WORK);
            let lcmap_path = o.path("lcmap", DEFAULT_LCMAP);
            let legend = landcover::read_veg_legend(&work.join("veg_legend.csv"))?;
            // class ids in the lc layers are 1 + the row of the table (see features::Ctx::new)
            let classes = landcover::classes(&lcmap_path, &legend)?;
            let deps = [lcmap_path.clone(), work.join("vec_net_struct.bin")];
            world::run(&hm, &o.path("features", DEFAULT_FEATURES), &work, &world_dir, &name, &rgen::SpecTable::new(&classes), &deps, &ro)?;
        }
        "base" => {
            // base --world DIR : fresh level.dat, data packs, icon and ATTRIBUTION (no regions)
            let world_dir = o.path("world", &format!(r"{DEFAULT_WORLD}\release_base\japan_dem_1m"));
            let name = o.kv.get("name").cloned().unwrap_or_else(|| "日本 1m DEM（静岡・山梨とその周辺）".to_string());
            world::write_base(&o.path("heightmap", DEFAULT_HEIGHTMAP), &world_dir, &name)?;
            eprintln!("base files written to {}", world_dir.display());
        }
        "verify" => verify::cmd(&args[2..])?,
        "hmprobe" => {
            // hmprobe X Z : value of the heightmap at a Minecraft column
            let out = o.path("out", DEFAULT_HEIGHTMAP);
            let x: i64 = o.pos[0].parse()?;
            let z: i64 = o.pos[1].parse()?;
            let (tx, tz) = (x.div_euclid(heightmap::TILE) * heightmap::TILE, z.div_euclid(heightmap::TILE) * heightmap::TILE);
            let hr = tiff::Reader::open(&out.join(heightmap::height_name(tx, tz)))?;
            let kr = tiff::Reader::open(&out.join(heightmap::class_name(tx, tz)))?;
            let (px, pz) = ((x - tx) as usize, (z - tz) as usize);
            let (bx, bz) = (px / tiff::TILE, pz / tiff::TILE);
            let ht = tiff::bytes_to_f32(hr.tile(bx, bz)?);
            let kt = kr.tile(bx, bz)?;
            let mut best = (f32::MIN, 0, 0);
            for dz in -3i64..=3 {
                let mut line = String::new();
                for dx in -3i64..=3 {
                    let (qx, qz) = (px as i64 % 512 + dx, pz as i64 % 512 + dz);
                    if qx < 0 || qz < 0 || qx >= 512 || qz >= 512 {
                        line.push_str("   ----- ");
                        continue;
                    }
                    let i = qz as usize * 512 + qx as usize;
                    line.push_str(&format!("{:8.2}{}", ht[i], kt[i]));
                    if ht[i] > best.0 {
                        best = (ht[i], x + dx, z + dz);
                    }
                }
                println!("{line}");
            }
            let i = (pz % 512) * 512 + px % 512;
            let e = ht[i];
            println!("X={x} Z={z}: elevation {e:.2} m, class {} ({}), Minecraft surface Y = {}", kt[i], heightmap::CLASS_NAMES[kt[i] as usize], -1760 + e.round() as i64);
            println!("highest in 7x7: {:.2} m at X={} Z={}", best.0, best.1, best.2);
        }
        "codestats" => {
            // counts of cell codes per level cache
            use rayon::prelude::*;
            let work = o.path("work", DEFAULT_WORK);
            for lv in [Lv::M1, Lv::M5, Lv::M10] {
                let st = store::LevelStore::open(lv, &work.join(format!("cache_{}.bin", lv.name())))?;
                let keys: Vec<u64> = st.keys().copied().collect();
                let c = keys
                    .par_iter()
                    .map(|k| {
                        let mut c = [0usize; 4];
                        for v in st.block(*k).unwrap() {
                            let i = if v.is_nan() {
                                0
                            } else if *v == gml::SEA {
                                1
                            } else if *v == gml::INLAND {
                                2
                            } else {
                                3
                            };
                            c[i] += 1;
                        }
                        c
                    })
                    .reduce(|| [0; 4], |a, b| [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]);
                println!("{}: blocks={} nodata={} sea={} inland={} elev={}", lv.name(), keys.len(), c[0], c[1], c[2], c[3]);
            }
        }
        "probe" => {
            // probe lat lon : print raw cells around the point for every level
            let work = o.path("work", DEFAULT_WORK);
            let lat: f64 = o.pos[0].parse()?;
            let lon: f64 = o.pos[1].parse()?;
            let p = proj::Lcc::new();
            let (x, z) = p.ll_to_mc(lat, lon);
            println!("MC X={x:.1} Z={z:.1}");
            for (lv, name) in [(Lv::M1, "cache_1m.bin"), (Lv::M5, "cache_5m.bin"), (Lv::M10, "cache_10m.bin"), (Lv::M5, "cache_waterlevel.bin")] {
                let st = store::LevelStore::open(lv, &work.join(name))?;
                let w = store::Window::new(&st, lat - 0.01, lat + 0.01, lon - 0.01, lon + 0.01);
                println!("{name}: sample={:?}", w.sample(lat, lon));
            }
        }
        "fgrid" => {
            // fgrid LAYER X Z W H : print a feature layer (U8) as a character grid
            let dir = o.path("features", DEFAULT_FEATURES);
            let (x0, z0, w, h): (i64, i64, i64, i64) = (o.pos[1].parse()?, o.pos[2].parse()?, o.pos[3].parse()?, o.pos[4].parse()?);
            let t = heightmap::TILE;
            let (tx, tz) = (x0.div_euclid(t), z0.div_euclid(t));
            let r = tiff::Reader::open(&dir.join(features::layer_name(&o.pos[0], tx * t, tz * t, "tif")))?;
            for z in z0..z0 + h {
                let mut line = String::new();
                for x in x0..x0 + w {
                    let (lx, lz) = ((x - tx * t) as usize, (z - tz * t) as usize);
                    let b = r.tile(lx / 512, lz / 512)?;
                    let v = b[(lz % 512) * 512 + lx % 512];
                    line.push(if v == 0 { '.' } else { char::from_digit(v as u32 % 36, 36).unwrap() });
                }
                println!("{line}");
            }
        }
        "vdump" => {
            // vdump FILE X Z R : features of a tiled vector store within R of (X, Z)
            let r = vfeat::TiledReader::open(&PathBuf::from(&o.pos[0]))?;
            let (x, z, rad): (f64, f64, f64) = (o.pos[1].parse()?, o.pos[2].parse()?, o.pos[3].parse()?);
            let (tx, tz) = ((x as i64).div_euclid(heightmap::TILE), (z as i64).div_euclid(heightmap::TILE));
            for f in r.tile(tx, tz) {
                let near = f.parts.iter().flatten().any(|p| (p[0] - x).hypot(p[1] - z) < rad);
                if near {
                    let n: usize = f.parts.iter().map(|p| p.len()).sum();
                    let p0 = f.parts[0][0];
                    let zr = f.zs.first().filter(|z| !z.is_empty()).map(|z| format!(" z[{:.1}..{:.1}] ends {:.1},{:.1}", z.iter().cloned().fold(f32::MAX, f32::min), z.iter().cloned().fold(f32::MIN, f32::max), z[0], z[z.len() - 1])).unwrap_or_default();
                    println!("id={} class={} flags={} nums={:?} area={} pts={} zs={}{zr} at=({:.0},{:.0}) text={}", f.id, f.class, f.flags, f.nums, f.area, n, f.zs.len(), p0[0], p0[1], f.text);
                }
            }
        }
        "proj" if o.pos.first().map(|s| s.as_str()) == Some("tm") => {
            // proj tm EAST NORTH : zone VIII plane coordinates -> lat/lon and MC
            let (e, nn): (f64, f64) = (o.pos[1].parse()?, o.pos[2].parse()?);
            let (la, lo) = proj::Tm::zone8().inverse(e, nn);
            let (x, z) = proj::Lcc::new().ll_to_mc(la, lo);
            println!("lat={la:.9} lon={lo:.9}  MC X={x:.3} Z={z:.3}");
        }
        "proj" => {
            let p = proj::Lcc::new();
            let lat: f64 = o.pos[0].parse()?;
            let lon: f64 = o.pos[1].parse()?;
            let (x, z) = p.ll_to_mc(lat, lon);
            let (la, lo) = p.mc_to_ll(x, z);
            println!("X={x:.4} Z={z:.4}  back: {la:.10} {lo:.10}");
        }
        _ => {
            eprintln!(
                "usage: jpdem <command> [--key value ...]\n\
                 \n\
                 pipeline (see README.md / build_all.ps1):\n\
                 \x20 scan       [--data E:\\data] [--work E:\\data\\mc1m_work] [--levels 1m,5m,10m] [--no-water] [--index-only]\n\
                 \x20 lakes      [--work ...] [--meshes 5137,5138,...]\n\
                 \x20 heightmap  [--work ...] [--out E:\\data\\heightmap] [--meshes ...] [--force] [--tiles \"X,Z;...\"] [--concurrency 4]\n\
                 \x20 kihon      [--data E:\\data] [--work ...]               基盤地図情報 基本項目 -> vec_kihon_*.bin\n\
                 \x20 vegmap     [--data ...] [--work ...]                  現存植生図 -> vec_veg.bin, veg_legend.csv\n\
                 \x20 plateau    [--plateau E:\\data\\PLATEAU] [--work ...]   PLATEAU bldg -> vec_plateau_bldg.bin\n\
                 \x20 osm        [PBF ...] [--work ...]                     OSM roads/rails/land use -> vec_osm_*.bin\n\
                 \x20 network    [--work ...] [--heightmap ...]             bridge/tunnel profiles -> vec_net_struct.bin\n\
                 \x20 trees      [--forest DIR] [--las DIR] [--no-forest] [--no-las]  Shizuoka tree points / LiDAR canopy -> trees\\\n\
                 \x20 lcmap      [--lcmap config\\landcover_map.csv]         create/refresh the land-cover mapping table\n\
                 \x20 features   [--out E:\\data\\features] [--tiles \"X,Z;...\"] [--concurrency 3]\n\
                 \x20 world      [--heightmap ...] [--features ...] [--world C:\\mc1m_world\\japan_dem_1m] [--force] [--level 6] [--around \"X,Z,R\"] [--regions \"rx,rz;...\"]\n\
                 \n\
                 checks:\n\
                 \x20 hmprobe X Z | proj LAT LON | proj tm EAST NORTH | probe LAT LON | codestats | nbtdump FILE | chunkdump REGION.mca LX LZ\n\
                 \x20 vdump VEC.bin X Z R | fgrid LAYER X Z W H\n\
                 \x20 verify striplight|lightcmp|blockcmp|column|render|featrender ..."
            );
            std::process::exit(2);
        }
    }
    Ok(())
}
