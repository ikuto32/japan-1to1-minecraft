//! `jpdem osmfind`: look up named OSM features (nodes, and ways by the mean
//! of their nodes) and print their position in lat/lon and Minecraft X/Z.
//! Used to place the landmarks of the in-game tools and the web pages.

use crate::proj::Lcc;
use anyhow::Result;
use osmpbf::{Element, ElementReader};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

const SHOW: &[&str] = &["railway", "public_transport", "natural", "tourism", "aeroway", "place", "man_made", "waterway", "historic", "amenity", "highway", "building", "leisure", "name:en"];

pub fn run(pbfs: &[PathBuf], names: &[String]) -> Result<()> {
    let want: HashSet<&str> = names.iter().map(|s| s.as_str()).collect();
    let lcc = Lcc::new();
    let found: Mutex<Vec<(String, String, Vec<(String, String)>, Option<(f64, f64)>, Vec<i64>)>> = Mutex::new(Vec::new());
    for pbf in pbfs {
        ElementReader::from_path(pbf)?.par_map_reduce(
            |el| {
                let (kind, id, tags, pos, refs): (&str, i64, Vec<(String, String)>, Option<(f64, f64)>, Vec<i64>) = match &el {
                    Element::Node(n) => ("node", n.id(), n.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect(), Some((n.lat(), n.lon())), vec![]),
                    Element::DenseNode(n) => ("node", n.id(), n.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect(), Some((n.lat(), n.lon())), vec![]),
                    Element::Way(w) => ("way", w.id(), w.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect(), None, w.refs().collect()),
                    _ => return,
                };
                let Some(name) = tags.iter().find(|(k, _)| k == "name").map(|(_, v)| v.clone()) else { return };
                if !want.contains(name.as_str()) {
                    return;
                }
                let shown: Vec<(String, String)> = tags.into_iter().filter(|(k, _)| SHOW.contains(&k.as_str())).collect();
                found.lock().unwrap().push((name, format!("{kind}/{id}"), shown, pos, refs));
            },
            || (),
            |_, _| (),
        )?;
    }
    let mut found = found.into_inner().unwrap();
    // way positions: mean of their nodes
    let need: HashSet<i64> = found.iter().flat_map(|f| f.4.iter().copied()).collect();
    if !need.is_empty() {
        let coords: Mutex<HashMap<i64, (f64, f64)>> = Mutex::new(HashMap::new());
        for pbf in pbfs {
            ElementReader::from_path(pbf)?.par_map_reduce(
                |el| {
                    let (id, lat, lon) = match el {
                        Element::Node(n) => (n.id(), n.lat(), n.lon()),
                        Element::DenseNode(n) => (n.id(), n.lat(), n.lon()),
                        _ => return,
                    };
                    if need.contains(&id) {
                        coords.lock().unwrap().insert(id, (lat, lon));
                    }
                },
                || (),
                |_, _| (),
            )?;
        }
        let coords = coords.into_inner().unwrap();
        for f in found.iter_mut() {
            if f.3.is_none() && !f.4.is_empty() {
                let pts: Vec<(f64, f64)> = f.4.iter().filter_map(|r| coords.get(r).copied()).collect();
                if !pts.is_empty() {
                    let n = pts.len() as f64;
                    f.3 = Some((pts.iter().map(|p| p.0).sum::<f64>() / n, pts.iter().map(|p| p.1).sum::<f64>() / n));
                }
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    found.dedup_by(|a, b| a.1 == b.1);
    for (name, id, tags, pos, _) in &found {
        let Some((lat, lon)) = pos else { continue };
        let (x, z) = lcc.ll_to_mc(*lat, *lon);
        let t: Vec<String> = tags.iter().map(|(k, v)| format!("{k}={v}")).collect();
        println!("{name}\t{id}\t{lat:.6}\t{lon:.6}\t{x:.0}\t{z:.0}\t{}", t.join(";"));
    }
    Ok(())
}
