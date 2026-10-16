//! OpenStreetMap (.osm.pbf) → tiled vector stores in Minecraft coordinates.
//!
//! Two passes over each PBF: (1) ways/relations with the tags we use, plus the
//! member ways of multipolygon relations; (2) the coordinates of the nodes
//! those ways reference. Ways become lines or closed rings; multipolygon
//! relations are stitched into rings. Features are split into category layers:
//! roads, rails, platforms, areas (land use etc.), structures.

use crate::area::Area;
use crate::ingest::TILE_MARGIN;
use crate::proj::Lcc;
use crate::vfeat::{self, VFeat};
use anyhow::Result;
use osmpbf::{Element, ElementReader};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// flags (all layers)
pub const F_BRIDGE: u16 = 1;
pub const F_TUNNEL: u16 = 2;
pub const F_AREA: u16 = 4;
pub const F_ONEWAY: u16 = 8;
pub const F_HIGHSPEED: u16 = 16;
pub const F_SERVICE: u16 = 32; // rail siding/yard/spur, road service
pub const F_SIDEWALK: u16 = 64; // footway=sidewalk / crossing
pub const F_EMBANKMENT: u16 = 128;
pub const F_CUTTING: u16 = 256;
pub const F_COVERED: u16 = 512;

pub const ROAD_CLASSES: &[&str] = &[
    "motorway", "motorway_link", "trunk", "trunk_link", "primary", "primary_link", "secondary", "secondary_link", "tertiary",
    "tertiary_link", "unclassified", "residential", "living_street", "service", "track", "road", "pedestrian", "footway",
    "path", "cycleway", "steps", "bridleway", "busway", "construction",
];
pub const RAIL_CLASSES: &[&str] = &["rail", "light_rail", "subway", "narrow_gauge", "monorail", "funicular", "tram", "preserved"];

/// Tags copied into `text` (k=v;k=v) for later classification.
const KEEP_TAGS: &[&str] = &[
    "highway", "railway", "building", "building:part", "landuse", "leisure", "amenity", "aeroway", "power", "man_made", "natural",
    "waterway", "crop", "produce", "trees", "orchard", "generator:source", "plant:source", "plant:method", "surface", "tracktype",
    "sac_scale", "footway", "service", "usage", "highspeed", "ref", "name", "roof:shape", "roof:colour", "building:colour",
    "building:material", "public_transport", "covered", "area", "bridge", "tunnel", "layer", "level", "location", "water", "wetland",
    "leaf_type", "leaf_cycle", "sport", "landcover", "tourism", "historic", "barrier", "embankment", "cutting", "width", "lanes",
    "building:levels", "height", "min_height", "building:min_level", "roof:levels", "roof:height", "est_width", "railway:track_ref", "indoor",
];

fn tag_text(tags: &[(String, String)]) -> String {
    let mut s = String::new();
    for (k, v) in tags {
        if KEEP_TAGS.contains(&k.as_str()) {
            if !s.is_empty() {
                s.push(';');
            }
            s.push_str(k);
            s.push('=');
            s.push_str(v);
        }
    }
    s
}

fn get<'a>(tags: &'a [(String, String)], k: &str) -> Option<&'a str> {
    tags.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str())
}

fn parse_num(v: Option<&str>) -> f32 {
    let Some(v) = v else { return 0.0 };
    let s: String = v.trim().chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
    s.parse().unwrap_or(0.0)
}

fn yes(v: Option<&str>) -> bool {
    matches!(v, Some(x) if x != "no" && x != "false" && x != "0")
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cat {
    Road,
    Rail,
    Platform,
    Building,
    Area,
    Struct,
}

const AREA_KEYS: &[&str] = &["landuse", "leisure", "amenity", "aeroway", "power", "man_made", "natural", "waterway", "tourism", "historic", "landcover"];

fn categorize(tags: &[(String, String)], closed: bool) -> Option<Cat> {
    if let Some(h) = get(tags, "highway") {
        if h == "platform" {
            return Some(Cat::Platform);
        }
        if ROAD_CLASSES.contains(&h) {
            return Some(Cat::Road);
        }
    }
    if let Some(r) = get(tags, "railway") {
        if r == "platform" {
            return Some(Cat::Platform);
        }
        if RAIL_CLASSES.contains(&r) {
            return Some(Cat::Rail);
        }
    }
    if get(tags, "public_transport") == Some("platform") && closed {
        return Some(Cat::Platform);
    }
    // OSM buildings are not used (PLATEAU / 基盤地図情報 only; keeps the ODbL
    // share-alike off the building layer). Building outlines are not land use.
    if let Some(b) = get(tags, "building").or_else(|| get(tags, "building:part")) {
        if b != "no" {
            return None;
        }
    }
    // structures (lines or areas): dams, weirs, piers, breakwaters, runways
    if matches!(get(tags, "waterway"), Some("dam" | "weir")) || matches!(get(tags, "man_made"), Some("pier" | "breakwater" | "groyne" | "bridge")) {
        return Some(Cat::Struct);
    }
    if matches!(get(tags, "aeroway"), Some("runway" | "taxiway")) && !closed {
        return Some(Cat::Struct);
    }
    if closed && AREA_KEYS.iter().any(|k| get(tags, k).is_some()) {
        return Some(Cat::Area);
    }
    None
}

fn class_of(cat: Cat, tags: &[(String, String)]) -> u16 {
    match cat {
        Cat::Road => get(tags, "highway").and_then(|h| ROAD_CLASSES.iter().position(|c| *c == h)).map(|p| p as u16 + 1).unwrap_or(99),
        Cat::Rail => get(tags, "railway").and_then(|h| RAIL_CLASSES.iter().position(|c| *c == h)).map(|p| p as u16 + 1).unwrap_or(99),
        _ => 0,
    }
}

fn flags_of(tags: &[(String, String)], closed_area: bool) -> u16 {
    let mut f = 0;
    if yes(get(tags, "bridge")) || get(tags, "man_made") == Some("bridge") {
        f |= F_BRIDGE;
    }
    if yes(get(tags, "tunnel")) || get(tags, "location") == Some("underground") {
        f |= F_TUNNEL;
    }
    if closed_area {
        f |= F_AREA;
    }
    if yes(get(tags, "oneway")) {
        f |= F_ONEWAY;
    }
    if yes(get(tags, "highspeed")) {
        f |= F_HIGHSPEED;
    }
    if get(tags, "service").is_some() {
        f |= F_SERVICE;
    }
    if matches!(get(tags, "footway"), Some("sidewalk" | "crossing")) {
        f |= F_SIDEWALK;
    }
    if yes(get(tags, "embankment")) {
        f |= F_EMBANKMENT;
    }
    if yes(get(tags, "cutting")) {
        f |= F_CUTTING;
    }
    if yes(get(tags, "covered")) {
        f |= F_COVERED;
    }
    f
}

fn nums_of(cat: Cat, tags: &[(String, String)]) -> [f32; 4] {
    let layer = parse_num(get(tags, "layer"));
    match cat {
        Cat::Road => [parse_num(get(tags, "width").or_else(|| get(tags, "est_width"))), parse_num(get(tags, "lanes")), layer, 0.0],
        Cat::Rail => [parse_num(get(tags, "tracks")), parse_num(get(tags, "gauge")), layer, 0.0],
        Cat::Building => [
            parse_num(get(tags, "building:levels")),
            parse_num(get(tags, "height")),
            parse_num(get(tags, "min_height")),
            parse_num(get(tags, "roof:levels")),
        ],
        _ => [0.0, 0.0, layer, 0.0],
    }
}

struct WayRec {
    id: i64,
    refs: Vec<i64>,
    tags: Vec<(String, String)>,
}

struct RelRec {
    id: i64,
    tags: Vec<(String, String)>,
    members: Vec<(i64, bool)>, // (way id, is_inner)
}

/// Close rings out of way segments (multipolygon assembly).
fn stitch(mut segs: Vec<Vec<i64>>) -> Vec<Vec<i64>> {
    let mut rings = Vec::new();
    while let Some(mut cur) = segs.pop() {
        let mut guard = 0;
        while cur.first() != cur.last() && guard < 100_000 {
            guard += 1;
            let end = *cur.last().unwrap();
            let Some(i) = segs.iter().position(|s| s.first() == Some(&end) || s.last() == Some(&end)) else { break };
            let mut s = segs.swap_remove(i);
            if s.first() != Some(&end) {
                s.reverse();
            }
            cur.extend_from_slice(&s[1..]);
        }
        if cur.len() >= 4 && cur.first() == cur.last() {
            rings.push(cur);
        }
    }
    rings
}

pub fn run(pbfs: &[PathBuf], work: &Path, area: &Area) -> Result<()> {
    let t0 = std::time::Instant::now();
    let lcc = Lcc::new();
    let mut ways: Vec<WayRec> = Vec::new();
    let mut rels: Vec<RelRec> = Vec::new();
    let mut seen_way: HashSet<i64> = HashSet::new();
    // pass 1: relations first (to learn member ways), then ways
    for pbf in pbfs {
        let rs: Mutex<Vec<RelRec>> = Mutex::new(Vec::new());
        ElementReader::from_path(pbf)?.for_each(|el| {
            if let Element::Relation(r) = el {
                let tags: Vec<(String, String)> = r.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                if get(&tags, "type") != Some("multipolygon") {
                    return;
                }
                if categorize(&tags, true).is_none() {
                    return;
                }
                let members: Vec<(i64, bool)> = r
                    .members()
                    .filter(|m| m.member_type == osmpbf::RelMemberType::Way)
                    .map(|m| (m.member_id, m.role().unwrap_or("") == "inner"))
                    .collect();
                rs.lock().unwrap().push(RelRec { id: r.id(), tags, members });
            }
        })?;
        rels.extend(rs.into_inner().unwrap());
    }
    let member_ways: HashSet<i64> = rels.iter().flat_map(|r| r.members.iter().map(|m| m.0)).collect();
    eprintln!("osm: {} multipolygon relations ({} member ways) in {:.1}s", rels.len(), member_ways.len(), t0.elapsed().as_secs_f64());
    for pbf in pbfs {
        let ws: Mutex<Vec<WayRec>> = Mutex::new(Vec::new());
        ElementReader::from_path(pbf)?.par_map_reduce(
            |el| {
                if let Element::Way(w) = el {
                    let refs: Vec<i64> = w.refs().collect();
                    let tags: Vec<(String, String)> = w.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                    let closed = refs.len() >= 4 && refs.first() == refs.last();
                    if categorize(&tags, closed).is_some() || member_ways.contains(&w.id()) {
                        ws.lock().unwrap().push(WayRec { id: w.id(), refs, tags });
                    }
                }
            },
            || (),
            |_, _| (),
        )?;
        for w in ws.into_inner().unwrap() {
            if seen_way.insert(w.id) {
                ways.push(w);
            }
        }
    }
    eprintln!("osm: {} ways kept in {:.1}s", ways.len(), t0.elapsed().as_secs_f64());
    // pass 2: node coordinates
    let mut need: Vec<i64> = ways.iter().flat_map(|w| w.refs.iter().copied()).collect();
    need.sort_unstable();
    need.dedup();
    let coords: Vec<std::sync::atomic::AtomicU64> = (0..need.len()).map(|_| std::sync::atomic::AtomicU64::new(u64::MAX)).collect();
    let enc = |lat: f64, lon: f64| -> u64 { (((lat * 1e7).round() as i32 as u32 as u64) << 32) | ((lon * 1e7).round() as i32 as u32 as u64) };
    for pbf in pbfs {
        ElementReader::from_path(pbf)?.par_map_reduce(
            |el| {
                let (id, lat, lon) = match el {
                    Element::Node(n) => (n.id(), n.lat(), n.lon()),
                    Element::DenseNode(n) => (n.id(), n.lat(), n.lon()),
                    _ => return,
                };
                if let Ok(i) = need.binary_search(&id) {
                    coords[i].store(enc(lat, lon), std::sync::atomic::Ordering::Relaxed);
                }
            },
            || (),
            |_, _| (),
        )?;
    }
    eprintln!("osm: {} nodes resolved in {:.1}s", need.len(), t0.elapsed().as_secs_f64());
    let node = |id: i64| -> Option<(f64, f64)> {
        let i = need.binary_search(&id).ok()?;
        let v = coords[i].load(std::sync::atomic::Ordering::Relaxed);
        if v == u64::MAX {
            return None;
        }
        Some(((v >> 32) as u32 as i32 as f64 / 1e7, (v as u32 as i32) as f64 / 1e7))
    };
    let (la0, la1, lo0, lo1) = (area.lat_min - 0.01, area.lat_max + 0.01, area.lon_min - 0.01, area.lon_max + 0.01);
    let to_mc = |refs: &[i64]| -> Option<Vec<[f64; 2]>> {
        let mut v = Vec::with_capacity(refs.len());
        let mut inside = false;
        for &r in refs {
            let (la, lo) = node(r)?;
            inside |= la >= la0 && la <= la1 && lo >= lo0 && lo <= lo1;
            let (x, z) = lcc.ll_to_mc(la, lo);
            v.push([x, z]);
        }
        inside.then_some(v)
    };
    let mut out: BTreeMap<&'static str, Vec<VFeat>> = BTreeMap::new();
    let cat_name = |c: Cat| match c {
        Cat::Road => "road",
        Cat::Rail => "rail",
        Cat::Platform => "platform",
        Cat::Building => "building",
        Cat::Area => "area",
        Cat::Struct => "struct",
    };
    let way_by_id: HashMap<i64, usize> = ways.iter().enumerate().map(|(i, w)| (w.id, i)).collect();
    let mut stats: BTreeMap<String, usize> = BTreeMap::new();
    for w in &ways {
        let closed = w.refs.len() >= 4 && w.refs.first() == w.refs.last();
        let Some(cat) = categorize(&w.tags, closed) else { continue };
        let Some(pts) = to_mc(&w.refs) else { continue };
        // a closed highway/railway is a loop line unless area=yes
        let is_area = closed && match cat {
            Cat::Road | Cat::Rail => get(&w.tags, "area") == Some("yes"),
            Cat::Platform => get(&w.tags, "area") != Some("no"),
            _ => true,
        };
        *stats.entry(cat_name(cat).to_string()).or_default() += 1;
        out.entry(cat_name(cat)).or_default().push(VFeat {
            class: class_of(cat, &w.tags),
            flags: flags_of(&w.tags, is_area),
            nums: nums_of(cat, &w.tags),
            id: w.id as u64,
            text: tag_text(&w.tags),
            area: is_area,
            parts: vec![pts],
            zs: vec![],
        });
    }
    for r in &rels {
        let Some(cat) = categorize(&r.tags, true) else { continue };
        let mut outer = Vec::new();
        let mut inner = Vec::new();
        for &(wid, is_inner) in &r.members {
            if let Some(&i) = way_by_id.get(&wid) {
                if is_inner {
                    inner.push(ways[i].refs.clone());
                } else {
                    outer.push(ways[i].refs.clone());
                }
            }
        }
        let outer = stitch(outer);
        let inner = stitch(inner);
        // each outer ring becomes one polygon with the inner rings it contains
        // (containment is resolved later by even-odd filling, so attach all)
        for o in outer {
            let Some(op) = to_mc(&o) else { continue };
            let mut parts = vec![op];
            for i in &inner {
                if let Some(ip) = to_mc(i) {
                    parts.push(ip);
                }
            }
            *stats.entry(format!("{}(rel)", cat_name(cat))).or_default() += 1;
            out.entry(cat_name(cat)).or_default().push(VFeat {
                class: class_of(cat, &r.tags),
                flags: flags_of(&r.tags, true),
                nums: nums_of(cat, &r.tags),
                id: (r.id as u64) | (1u64 << 62),
                text: tag_text(&r.tags),
                area: true,
                parts,
                zs: vec![],
            });
        }
    }
    for (name, feats) in &out {
        let p = work.join(format!("vec_osm_{name}.bin"));
        vfeat::write_tiled(&p, feats, TILE_MARGIN)?;
        eprintln!("osm: {name}: {} features -> {}", feats.len(), p.display());
    }
    eprintln!("osm: {:?}", stats);
    eprintln!("osm: done in {:.1}s", t0.elapsed().as_secs_f64());
    Ok(())
}
