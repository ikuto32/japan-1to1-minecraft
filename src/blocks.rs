//! Block-state registry for the world writer: name + properties + the
//! flags needed for Minecraft heightmaps. Ids are u16 indices into a table
//! that is built once (`Registry::get()`); new states are interned on demand.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

pub type BlockId = u16;

#[derive(Clone, Debug)]
pub struct BlockDef {
    pub name: String,
    pub props: Vec<(String, String)>,
    /// counts for MOTION_BLOCKING / OCEAN_FLOOR (vanilla "legacy solid")
    pub motion: bool,
    pub fluid: bool,
    pub leaves: bool,
    /// how it passes sky light
    pub light: LightProp,
    /// serialized palette entry (compound payload)
    pub nbt: Vec<u8>,
}

/// How a block passes sky light in Minecraft's light engine: the light it
/// takes away (`opacity`: 0 clear, 1 water and leaves, 15 solid) and, for
/// the blocks whose shape closes faces to light (slabs, farmland, paths),
/// the faces it closes: the bottom or the top face whole, the side faces over
/// a height range in 16ths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightProp {
    pub opacity: u8,
    pub down: bool,
    pub up: bool,
    pub side: (u8, u8),
}

const CLEAR: LightProp = LightProp { opacity: 0, down: false, up: false, side: (0, 0) };
const OPAQUE: LightProp = LightProp { opacity: 15, down: false, up: false, side: (0, 0) };

fn light_prop(name: &str, props: &[(String, String)]) -> LightProp {
    let n = name.trim_start_matches("minecraft:");
    let prop = |k: &str| props.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
    // a waterlogged block holds water, which takes light away
    let wet = if prop("waterlogged") == Some("true") { 1 } else { 0 };
    let shaped = |down: bool, up: bool, side: (u8, u8)| LightProp { opacity: wet, down, up, side };
    match n {
        "air" | "cave_air" | "void_air" => CLEAR,
        "water" | "seagrass" | "tall_seagrass" | "kelp" | "kelp_plant" | "bubble_column" => LightProp { opacity: 1, ..CLEAR },
        _ if n.ends_with("_leaves") => LightProp { opacity: 1, ..CLEAR },
        _ if n.ends_with("_slab") => match prop("type") {
            Some("double") => OPAQUE,
            Some("top") => shaped(false, true, (8, 16)),
            _ => shaped(true, false, (0, 8)),
        },
        "farmland" | "dirt_path" => shaped(true, false, (0, 15)),
        "daylight_detector" => shaped(true, false, (0, 6)),
        "snow" => {
            let l: u8 = prop("layers").and_then(|v| v.parse().ok()).unwrap_or(1);
            if l >= 8 { OPAQUE } else { shaped(true, false, (0, 2 * l)) }
        }
        "glass" | "iron_bars" | "bamboo" | "chain" => LightProp { opacity: wet, ..CLEAR },
        _ if n.ends_with("_stained_glass") || n.ends_with("_pane") || n.ends_with("_wall") || n.ends_with("_fence") || n.ends_with("_fence_gate") => {
            LightProp { opacity: wet, ..CLEAR }
        }
        // plants, rails, carpets, torches: no collision, light passes
        _ if !is_motion_blocking(name) => LightProp { opacity: wet, ..CLEAR },
        _ => OPAQUE,
    }
}

pub struct Registry {
    defs: RwLock<Vec<BlockDef>>,
    index: RwLock<HashMap<String, BlockId>>,
}

/// Non-solid blocks for heightmap purposes (no collision or very thin).
fn is_motion_blocking(name: &str) -> bool {
    let n = name.trim_start_matches("minecraft:");
    !(n == "air"
        || n == "water"
        || n.ends_with("rail")
        || n.ends_with("carpet")
        || n.ends_with("_sapling")
        || n == "short_grass"
        || n == "tall_grass"
        || n == "fern"
        || n == "large_fern"
        || n == "dead_bush"
        || n == "seagrass"
        || n == "tall_seagrass"
        || n == "lily_pad"
        || n == "wheat"
        || n == "carrots"
        || n == "potatoes"
        || n == "beetroots"
        || n == "sweet_berry_bush"
        || n == "torch"
        || n == "ladder"
        || n == "snow"
        || n.ends_with("_tulip")
        || matches!(n, "poppy" | "dandelion" | "azure_bluet" | "oxeye_daisy" | "cornflower" | "lily_of_the_valley" | "allium" | "blue_orchid" | "pink_petals" | "bush"))
}

fn encode(name: &str, props: &[(String, String)]) -> Vec<u8> {
    let mut w = crate::nbt::W::new();
    w.string("Name", name);
    if !props.is_empty() {
        w.begin_compound("Properties");
        for (k, v) in props {
            w.string(k, v);
        }
        w.end();
    }
    w.end();
    w.buf
}

impl Registry {
    pub fn get() -> &'static Registry {
        static R: OnceLock<Registry> = OnceLock::new();
        R.get_or_init(|| {
            let r = Registry { defs: RwLock::new(Vec::new()), index: RwLock::new(HashMap::new()) };
            // id 0 must be air
            r.id("minecraft:air");
            r
        })
    }

    /// Intern a block state written as `name` or `name[k=v,k=v]`.
    pub fn id(&self, spec: &str) -> BlockId {
        if let Some(&i) = self.index.read().unwrap().get(spec) {
            return i;
        }
        let (name, props) = match spec.find('[') {
            Some(p) => {
                let props: Vec<(String, String)> = spec[p + 1..spec.len() - 1]
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|kv| {
                        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                        (k.trim().to_string(), v.trim().to_string())
                    })
                    .collect();
                (spec[..p].to_string(), props)
            }
            None => (spec.to_string(), vec![]),
        };
        let name = if name.contains(':') { name } else { format!("minecraft:{name}") };
        let mut idx = self.index.write().unwrap();
        if let Some(&i) = idx.get(spec) {
            return i;
        }
        let mut defs = self.defs.write().unwrap();
        let i = defs.len() as BlockId;
        let short = name.trim_start_matches("minecraft:").to_string();
        defs.push(BlockDef {
            motion: is_motion_blocking(&name),
            fluid: short == "water" || short == "seagrass" || short == "tall_seagrass" || props.iter().any(|(k, v)| k == "waterlogged" && v == "true"),
            leaves: short.ends_with("_leaves"),
            light: light_prop(&name, &props),
            nbt: encode(&name, &props),
            name,
            props,
        });
        idx.insert(spec.to_string(), i);
        i
    }

    pub fn def(&self, id: BlockId) -> BlockDef {
        self.defs.read().unwrap()[id as usize].clone()
    }

    /// Snapshot of all definitions (index = id), for fast lookups in hot loops.
    pub fn snapshot(&self) -> Vec<BlockDef> {
        self.defs.read().unwrap().clone()
    }
}

/// Shorthand: intern a block spec.
pub fn b(spec: &str) -> BlockId {
    Registry::get().id(spec)
}
