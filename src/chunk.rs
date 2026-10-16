//! Region assembly and chunk serialization for the world writer:
//! feature-layer windows, the generator passes, heightmaps, and sky light
//! (precomputed for plain terrain chunks; chunks containing structures,
//! plants or trees are saved unlit so Minecraft computes their light).

use crate::blocks::{BlockDef, BlockId, Registry};
use crate::features::{self, BldRec};
use crate::heightmap::TILE;
use crate::nbt::W;
use crate::rgen::{self, Col, FeatWin, Gen, Ids, Overlay, SpecTable, MIN_Y};
use crate::tiff::{self, Fmt};
use crate::vfeat::VFeat;
use crate::world::RegionInput;
use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub const DATA_VERSION: i32 = 4671;
pub const SEC_MIN: i32 = MIN_Y / 16;
pub const N_SEC: i32 = 254;
pub const HALO: usize = 32;
pub const RW: usize = 512 + 2 * HALO;

// ---------------------------------------------------------------- feature layers

type Shared<T> = Arc<OnceLock<Option<Arc<T>>>>;

/// One decoded 512 block of every feature layer.
pub struct FeatBlock {
    pub lc: Vec<u16>,
    pub bldg: Vec<u32>,
    pub roof: Vec<f32>,
    pub strct: Vec<u8>,
    pub road: Vec<u8>,
    pub roadh: Vec<f32>,
    pub rail: Vec<u8>,
    pub railh: Vec<f32>,
    pub tcov: Vec<u8>,
    pub lake: Vec<f32>,
    pub rowdir: Vec<u8>,
    pub canopy: Vec<u8>,
}

pub struct TileReaders {
    layers: HashMap<&'static str, tiff::Reader>,
    table: Vec<BldRec>,
    /// measured trees (absolute X, Z, height m, species code)
    trees: Vec<(i64, i64, f32, u8)>,
}

pub struct FeatSet {
    dir: PathBuf,
    tiles: Mutex<HashMap<(i64, i64), Shared<TileReaders>>>,
    blocks: Mutex<HashMap<(i64, i64), Shared<FeatBlock>>>,
}

const LAYERS: [&str; 12] = ["lc", "bldg", "roof", "struct", "road", "roadh", "rail", "railh", "tcov", "lake", "rowdir", "canopy"];

impl FeatSet {
    pub fn new(dir: &Path) -> Self {
        FeatSet { dir: dir.to_path_buf(), tiles: Mutex::new(HashMap::new()), blocks: Mutex::new(HashMap::new()) }
    }

    fn tile(&self, tx: i64, tz: i64) -> Option<Arc<TileReaders>> {
        let cell = {
            let mut m = self.tiles.lock().unwrap();
            if m.len() > 24 && !m.contains_key(&(tx, tz)) {
                m.clear();
            }
            m.entry((tx, tz)).or_default().clone()
        };
        cell.get_or_init(|| {
            let (x, z) = (tx * TILE, tz * TILE);
            let mut layers = HashMap::new();
            for l in LAYERS {
                if let Ok(r) = tiff::Reader::open(&self.dir.join(features::layer_name(l, x, z, "tif"))) {
                    layers.insert(l, r);
                }
            }
            if layers.is_empty() {
                return None;
            }
            let table = features::read_bldg_table(&self.dir.join(features::layer_name("bldg", x, z, "bin")));
            let trees = std::fs::read(self.dir.join(features::layer_name("trees", x, z, "bin")))
                .map(|d| {
                    d.chunks_exact(8)
                        .map(|c| {
                            let lx = u16::from_le_bytes([c[0], c[1]]) as i64;
                            let lz = u16::from_le_bytes([c[2], c[3]]) as i64;
                            (x + lx, z + lz, u16::from_le_bytes([c[4], c[5]]) as f32 / 10.0, c[6])
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(Arc::new(TileReaders { layers, table, trees }))
        })
        .clone()
    }

    /// Building table of a tile (for building keys).
    pub fn table(&self, tx: i64, tz: i64) -> Option<Arc<TileReaders>> {
        self.tile(tx, tz)
    }

    fn block(&self, bx: i64, bz: i64) -> Option<Arc<FeatBlock>> {
        let cell = {
            let mut m = self.blocks.lock().unwrap();
            if m.len() > 2000 && !m.contains_key(&(bx, bz)) {
                m.clear();
            }
            m.entry((bx, bz)).or_default().clone()
        };
        cell.get_or_init(|| {
            let per = TILE / 512;
            let t = self.tile(bx.div_euclid(per), bz.div_euclid(per))?;
            let (ix, iz) = (bx.rem_euclid(per) as usize, bz.rem_euclid(per) as usize);
            let rd = |l: &str| -> Option<Vec<u8>> { t.layers.get(l).and_then(|r| r.tile(ix, iz).ok()) };
            let u16v = |v: Option<Vec<u8>>| v.map(|b| b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()).unwrap_or_else(|| vec![0u16; 512 * 512]);
            let u32v = |v: Option<Vec<u8>>| v.map(|b| b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()).unwrap_or_else(|| vec![0u32; 512 * 512]);
            let f32v = |v: Option<Vec<u8>>| v.map(tiff::bytes_to_f32).unwrap_or_else(|| vec![f32::NAN; 512 * 512]);
            let u8v = |v: Option<Vec<u8>>| v.unwrap_or_else(|| vec![0u8; 512 * 512]);
            Some(Arc::new(FeatBlock {
                lc: u16v(rd("lc")),
                bldg: u32v(rd("bldg")),
                roof: f32v(rd("roof")),
                strct: u8v(rd("struct")),
                road: u8v(rd("road")),
                roadh: f32v(rd("roadh")),
                rail: u8v(rd("rail")),
                railh: f32v(rd("railh")),
                tcov: u8v(rd("tcov")),
                lake: f32v(rd("lake")),
                rowdir: u8v(rd("rowdir")),
                canopy: u8v(rd("canopy")),
            }))
        })
        .clone()
    }

    /// Feature window matching `RegionInput` (RW × RW at ox, oz).
    pub fn window(&self, ox: i64, oz: i64) -> FeatWin {
        let n = RW;
        let mut fw = FeatWin {
            lc: vec![0; n * n],
            bldg: vec![0; n * n],
            roof: vec![f32::NAN; n * n],
            strct: vec![0; n * n],
            road: vec![0; n * n],
            roadh: vec![f32::NAN; n * n],
            rail: vec![0; n * n],
            railh: vec![f32::NAN; n * n],
            tcov: vec![0; n * n],
            lake: vec![f32::NAN; n * n],
            rowdir: vec![0; n * n],
            canopy: vec![0; n * n],
            trees: Vec::new(),
            bld: vec![BldRec::default()],
        };
        let mut offsets: HashMap<(i64, i64), u32> = HashMap::new();
        let per = TILE / 512;
        for bz in oz.div_euclid(512)..=(oz + n as i64 - 1).div_euclid(512) {
            for bx in ox.div_euclid(512)..=(ox + n as i64 - 1).div_euclid(512) {
                let Some(b) = self.block(bx, bz) else { continue };
                let tkey = (bx.div_euclid(per), bz.div_euclid(per));
                let off = match offsets.get(&tkey) {
                    Some(&o) => o,
                    None => {
                        let o = fw.bld.len() as u32 - 1;
                        if let Some(t) = self.tile(tkey.0, tkey.1) {
                            fw.bld.extend_from_slice(&t.table);
                            let (x1, z1) = (ox + n as i64, oz + n as i64);
                            fw.trees.extend(t.trees.iter().filter(|p| p.0 >= ox && p.0 < x1 && p.1 >= oz && p.1 < z1).copied());
                        }
                        offsets.insert(tkey, o);
                        o
                    }
                };
                let (zs, ze) = ((bz * 512).max(oz), ((bz + 1) * 512).min(oz + n as i64));
                let (xs, xe) = ((bx * 512).max(ox), ((bx + 1) * 512).min(ox + n as i64));
                for z in zs..ze {
                    for x in xs..xe {
                        let s = ((z - bz * 512) * 512 + (x - bx * 512)) as usize;
                        let d = ((z - oz) as usize) * n + (x - ox) as usize;
                        fw.lc[d] = b.lc[s];
                        fw.bldg[d] = if b.bldg[s] != 0 { b.bldg[s] + off } else { 0 };
                        fw.roof[d] = b.roof[s];
                        fw.strct[d] = b.strct[s];
                        fw.road[d] = b.road[s];
                        fw.roadh[d] = b.roadh[s];
                        fw.rail[d] = b.rail[s];
                        fw.railh[d] = b.railh[s];
                        fw.tcov[d] = b.tcov[s];
                        fw.lake[d] = b.lake[s];
                        fw.rowdir[d] = b.rowdir[s];
                        fw.canopy[d] = b.canopy[s];
                    }
                }
            }
        }
        fw
    }

    /// Modification time of every feature layer of a tile (max), for incremental builds.
    pub fn mtime(&self, tx: i64, tz: i64) -> Option<std::time::SystemTime> {
        let (x, z) = (tx * TILE, tz * TILE);
        let mut m: Option<std::time::SystemTime> = None;
        for l in LAYERS {
            if let Ok(t) = std::fs::metadata(self.dir.join(features::layer_name(l, x, z, "tif"))).and_then(|md| md.modified()) {
                m = Some(m.map_or(t, |v| v.max(t)));
            }
        }
        for l in ["bldg", "trees"] {
            if let Ok(t) = std::fs::metadata(self.dir.join(features::layer_name(l, x, z, "bin"))).and_then(|md| md.modified()) {
                m = Some(m.map_or(t, |v| v.max(t)));
            }
        }
        m
    }
}

// ---------------------------------------------------------------- sky light (terrain chunks)

fn water_light_extras(cols: &[Col]) -> HashMap<u32, Vec<(i32, u8)>> {
    let n = RW;
    let vert = |c: &Col, y: i32| -> u8 { (15 - (c.w - y + 1)).max(0) as u8 };
    let mut extra: HashMap<(u32, i32), u8> = HashMap::new();
    let mut queue: Vec<Vec<(u32, i32)>> = vec![Vec::new(); 16];
    for z in 0..n {
        for x in 0..n {
            let i = z * n + x;
            let c = &cols[i];
            if c.out || c.w <= c.g {
                continue;
            }
            for (dx, dz) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                let (xx, zz) = (x as i64 + dx, z as i64 + dz);
                if xx < 0 || zz < 0 || xx >= n as i64 || zz >= n as i64 {
                    continue;
                }
                let nb = &cols[zz as usize * n + xx as usize];
                let y0 = if nb.out { MIN_Y } else { nb.top_y() + 1 };
                for y in y0.max(c.g + 1)..=c.w {
                    if 14 > vert(c, y) {
                        let e = extra.entry((i as u32, y)).or_insert(0);
                        if *e < 14 {
                            *e = 14;
                            queue[14].push((i as u32, y));
                        }
                    }
                }
            }
        }
    }
    for lvl in (2..=14u8).rev() {
        let items = std::mem::take(&mut queue[lvl as usize]);
        for (i, y) in items {
            if extra.get(&(i, y)).copied().unwrap_or(0) != lvl {
                continue;
            }
            let (x, z) = (i as usize % n, i as usize / n);
            let nl = lvl - 1;
            let try_push = |j: usize, yy: i32, extra: &mut HashMap<(u32, i32), u8>, queue: &mut Vec<Vec<(u32, i32)>>| {
                let c = &cols[j];
                if c.out || yy <= c.g || yy > c.w || nl <= vert(c, yy) {
                    return;
                }
                let e = extra.entry((j as u32, yy)).or_insert(0);
                if *e < nl {
                    *e = nl;
                    queue[nl as usize].push((j as u32, yy));
                }
            };
            try_push(i as usize, y - 1, &mut extra, &mut queue);
            try_push(i as usize, y + 1, &mut extra, &mut queue);
            if x > 0 {
                try_push(i as usize - 1, y, &mut extra, &mut queue);
            }
            if x + 1 < n {
                try_push(i as usize + 1, y, &mut extra, &mut queue);
            }
            if z > 0 {
                try_push(i as usize - n, y, &mut extra, &mut queue);
            }
            if z + 1 < n {
                try_push(i as usize + n, y, &mut extra, &mut queue);
            }
        }
    }
    let mut out: HashMap<u32, Vec<(i32, u8)>> = HashMap::new();
    for ((i, y), l) in extra {
        out.entry(i).or_default().push((y, l));
    }
    out
}

// ---------------------------------------------------------------- chunk NBT

fn pack(indices: &[u16], bits: usize) -> Vec<i64> {
    let per = 64 / bits;
    let mut out = vec![0i64; indices.len().div_ceil(per)];
    for (i, &v) in indices.iter().enumerate() {
        out[i / per] |= (v as i64) << ((i % per) * bits);
    }
    out
}

fn bits_for(n: usize) -> usize {
    (usize::BITS - (n - 1).leading_zeros()) as usize
}

pub struct Pre {
    pub defs: Vec<BlockDef>,
    uniform: HashMap<BlockId, Vec<u8>>,
}

impl Pre {
    pub fn new() -> Self {
        Pre { defs: Registry::get().snapshot(), uniform: HashMap::new() }
    }
    fn def(&mut self, id: BlockId) -> &BlockDef {
        if id as usize >= self.defs.len() {
            self.defs = Registry::get().snapshot();
        }
        &self.defs[id as usize]
    }
    fn uniform_states(&mut self, id: BlockId) -> Vec<u8> {
        if let Some(v) = self.uniform.get(&id) {
            return v.clone();
        }
        let payload = self.def(id).nbt.clone();
        let mut w = W::new();
        w.begin_compound("block_states");
        w.begin_list("palette", 10, 1);
        w.buf.extend_from_slice(&payload);
        w.end();
        self.uniform.insert(id, w.buf.clone());
        w.buf
    }
}

/// Serialize one chunk. `cols`/`ovs` are the 256 columns (index z*16+x).
pub fn chunk_nbt(pre: &mut Pre, cx: i32, cz: i32, cols: &[Col; 256], ovs: &[&[(i32, BlockId)]; 256], extras: &[Option<&Vec<(i32, u8)>>; 256], biome_names: &[String]) -> Vec<u8> {
    let ids = Ids::get();
    let mut w = W::new();
    w.begin_root();
    w.int("DataVersion", DATA_VERSION);
    w.int("xPos", cx);
    w.int("yPos", SEC_MIN);
    w.int("zPos", cz);
    w.string("Status", "minecraft:full");
    w.long("LastUpdate", 0);
    w.long("InhabitedTime", 0);
    let has_ov = ovs.iter().any(|o| !o.is_empty());
    // structures/plants: let Minecraft light the chunk on load
    w.byte("isLightOn", if has_ov { 0 } else { 1 });
    let any: Vec<&Col> = cols.iter().filter(|c| !c.out).collect();
    let ov_max = ovs.iter().flat_map(|o| o.iter().map(|e| e.0)).max().unwrap_or(MIN_Y);
    let ov_min = ovs.iter().flat_map(|o| o.iter().map(|e| e.0)).min().unwrap_or(i32::MAX);
    let top = any.iter().map(|c| c.top_y()).max().unwrap_or(MIN_Y).max(ov_max);
    let top_sec = top.div_euclid(16);
    let has_out = cols.iter().any(|c| c.out);
    let low_open = if has_out { MIN_Y + 1 } else { any.iter().map(|c| c.g + 1).min().unwrap_or(MIN_Y) };
    let low_open_sec = low_open.div_euclid(16);
    let layer_low = any.iter().map(|c| c.layer_bottom()).min().unwrap_or(MIN_Y);
    let max_g = any.iter().map(|c| c.g).max().unwrap_or(MIN_Y);
    let min_w = any.iter().map(|c| c.w).min().unwrap_or(MIN_Y);
    // biomes (4×4 cells, same in every section)
    let mut bpal: Vec<u16> = Vec::new();
    let mut bidx = [0u16; 16];
    for qz in 0..4 {
        for qx in 0..4 {
            let c = &cols[(qz * 4 + 2) * 16 + qx * 4 + 2];
            let bi = if c.out { 0 } else { c.biome };
            let p = match bpal.iter().position(|&x| x == bi) {
                Some(p) => p,
                None => {
                    bpal.push(bi);
                    bpal.len() - 1
                }
            };
            bidx[qz * 4 + qx] = p as u16;
        }
    }
    let biome_bytes = {
        let mut bw = W::new();
        bw.begin_compound("biomes");
        bw.begin_list("palette", 8, bpal.len());
        for &x in &bpal {
            bw.str_payload(&biome_names[x as usize]);
        }
        if bpal.len() > 1 {
            let mut idx = [0u16; 64];
            for y in 0..4 {
                for i in 0..16 {
                    idx[y * 16 + i] = bidx[i];
                }
            }
            bw.long_array("data", &pack(&idx, bits_for(bpal.len())));
        }
        bw.end();
        bw.buf
    };
    w.begin_list("sections", 10, N_SEC as usize);
    let mut blocks = vec![0u16; 4096];
    let mut light = vec![0u8; 2048];
    for sy in SEC_MIN..SEC_MIN + N_SEC {
        let y0 = sy * 16;
        let y1 = y0 + 15;
        w.byte("Y", sy as i8);
        let ov_here = has_ov && y1 >= ov_min && y0 <= ov_max;
        let uniform: Option<BlockId> = if ov_here {
            None
        } else if y0 > top {
            Some(ids.air)
        } else if !has_out && y1 < layer_low && y0 > MIN_Y {
            Some(ids.stone)
        } else if !has_out && max_g < y0 && min_w >= y1 {
            Some(ids.water)
        } else {
            None
        };
        match uniform {
            Some(bk) => {
                let u = pre.uniform_states(bk);
                w.buf.extend_from_slice(&u);
            }
            None => {
                let mut pal: Vec<BlockId> = Vec::with_capacity(8);
                let mut map: HashMap<BlockId, u16> = HashMap::new();
                for i in 0..256 {
                    let c = &cols[i];
                    let ov = ovs[i];
                    for ly in 0..16 {
                        let y = y0 + ly;
                        let mut bk = c.block(y, ids);
                        if ov_here {
                            if let Some(e) = ov.iter().find(|e| e.0 == y) {
                                bk = e.1;
                            }
                        }
                        let p = *map.entry(bk).or_insert_with(|| {
                            pal.push(bk);
                            (pal.len() - 1) as u16
                        });
                        blocks[ly as usize * 256 + i] = p;
                    }
                }
                w.begin_compound("block_states");
                w.begin_list("palette", 10, pal.len());
                for &bk in &pal {
                    let payload = pre.def(bk).nbt.clone();
                    w.buf.extend_from_slice(&payload);
                }
                if pal.len() > 1 {
                    w.long_array("data", &pack(&blocks, bits_for(pal.len()).max(4)));
                }
                w.end();
            }
        }
        w.buf.extend_from_slice(&biome_bytes);
        if !has_ov && sy >= low_open_sec && sy <= top_sec + 1 {
            if y0 > top {
                light.fill(0xff);
            } else {
                light.fill(0);
                #[inline]
                fn set(light: &mut [u8], idx: usize, v: u8) {
                    light[idx >> 1] |= if idx & 1 == 0 { v } else { v << 4 };
                }
                for i in 0..256 {
                    let c = &cols[i];
                    if c.out {
                        for ly in 0..16 {
                            set(&mut light, ly * 256 + i, 15);
                        }
                        continue;
                    }
                    for y in (c.top_y() + 1).max(y0)..=y1 {
                        set(&mut light, (y - y0) as usize * 256 + i, 15);
                    }
                    if c.w > c.g {
                        for y in (c.g + 1).max(y0).max(c.w - 13)..=c.w.min(y1) {
                            set(&mut light, (y - y0) as usize * 256 + i, (14 - (c.w - y)) as u8);
                        }
                    }
                    if let Some(ex) = extras[i] {
                        for &(y, l) in ex.iter() {
                            if y >= y0 && y <= y1 {
                                let idx = (y - y0) as usize * 256 + i;
                                let bt = &mut light[idx >> 1];
                                if idx & 1 == 0 {
                                    if l > *bt & 0x0f {
                                        *bt = (*bt & 0xf0) | l;
                                    }
                                } else if l > *bt >> 4 {
                                    *bt = (*bt & 0x0f) | (l << 4);
                                }
                            }
                        }
                    }
                }
            }
            w.byte_array("SkyLight", &light);
        }
        w.end();
    }
    // heightmaps from the final column content
    let mut surf = vec![0u16; 256];
    let mut mb = vec![0u16; 256];
    let mut mbnl = vec![0u16; 256];
    let mut floor = vec![0u16; 256];
    for i in 0..256 {
        let c = &cols[i];
        if c.out && ovs[i].is_empty() {
            continue;
        }
        let base_top = if c.out { MIN_Y - 1 } else { c.top_y() };
        let ov_top = ovs[i].iter().filter(|e| e.1 != ids.air).map(|e| e.0).max().unwrap_or(MIN_Y - 1);
        let hi = base_top.max(ov_top);
        let (mut s, mut m, mut mn, mut f) = (None, None, None, None);
        let mut y = hi;
        while y >= MIN_Y && (s.is_none() || m.is_none() || mn.is_none() || f.is_none()) {
            let mut bk = if c.out { ids.air } else { c.block(y, ids) };
            if let Some(e) = ovs[i].iter().find(|e| e.0 == y) {
                bk = e.1;
            }
            if bk != ids.air {
                let d = pre.def(bk);
                let (motion, fluid, lv) = (d.motion, d.fluid, d.leaves);
                if s.is_none() {
                    s = Some(y);
                }
                if m.is_none() && (motion || fluid) {
                    m = Some(y);
                }
                if mn.is_none() && (motion || fluid) && !lv {
                    mn = Some(y);
                }
                if f.is_none() && motion && !fluid {
                    f = Some(y);
                }
            }
            if y <= base_top - 1 && s.is_some() && m.is_some() && mn.is_some() && f.is_none() {
                // below the surface everything is solid
            }
            y -= 1;
        }
        let v = |o: Option<i32>| o.map(|y| (y + 1 - MIN_Y) as u16).unwrap_or(0);
        surf[i] = v(s);
        mb[i] = v(m);
        mbnl[i] = v(mn);
        floor[i] = v(f);
    }
    w.begin_compound("Heightmaps");
    w.long_array("MOTION_BLOCKING", &pack(&mb, 12));
    w.long_array("MOTION_BLOCKING_NO_LEAVES", &pack(&mbnl, 12));
    w.long_array("OCEAN_FLOOR", &pack(&floor, 12));
    w.long_array("WORLD_SURFACE", &pack(&surf, 12));
    w.end();
    w.begin_list("block_entities", 10, 0);
    w.begin_list("block_ticks", 10, 0);
    w.begin_list("fluid_ticks", 10, 0);
    w.begin_list("PostProcessing", 9, N_SEC as usize);
    for _ in 0..N_SEC {
        w.buf.push(0);
        w.buf.extend_from_slice(&0i32.to_be_bytes());
    }
    w.begin_compound("structures");
    w.begin_compound("References");
    w.end();
    w.begin_compound("starts");
    w.end();
    w.end();
    w.end();
    w.buf
}

/// Everything a region build needs besides the terrain window.
pub struct Extra<'a> {
    pub fw: FeatWin,
    pub st: &'a SpecTable,
    pub structs: Vec<VFeat>,
}

/// Build one region file (None if no chunk of it lies inside the area).
pub fn build_region(inp: &RegionInput, ex: &Extra, rx: i64, rz: i64, level: u32) -> Result<Option<(Vec<u8>, usize)>> {
    let g = Gen { n: RW, ox: inp.ox, oz: inp.oz, h: &inp.h, k: &inp.k, fx: &ex.fw, st: ex.st };
    let mut cols = g.columns();
    let mut ov = Overlay::new(RW * RW);
    let decks = crate::infra::apply(&g, &mut cols, &mut ov, &ex.structs);
    rgen::buildings(&g, &mut cols, &mut ov, &decks);
    rgen::patterns(&g, &mut cols, &mut ov);
    rgen::trees(&g, &cols, &mut ov);
    rgen::plants(&g, &cols, &mut ov);
    ov.finish();
    let extras = water_light_extras(&cols);
    let mut pre = Pre::new();
    let mut comp = libdeflater::Compressor::new(libdeflater::CompressionLvl::new(level as i32).unwrap_or_default());
    let mut cbuf: Vec<u8> = Vec::new();
    let mut chunks: Vec<Option<Vec<u8>>> = vec![None; 1024];
    let mut nchunks = 0;
    for lz in 0..32 {
        for lx in 0..32 {
            let mut cc = [rgen::OUT_COL; 256];
            let mut ovs: [&[(i32, BlockId)]; 256] = [&[]; 256];
            let mut exs: [Option<&Vec<(i32, u8)>>; 256] = [None; 256];
            let mut any = false;
            for z in 0..16 {
                for x in 0..16 {
                    let wi = (HALO + lz * 16 + z) * RW + HALO + lx * 16 + x;
                    cc[z * 16 + x] = cols[wi];
                    ovs[z * 16 + x] = &ov.v[wi];
                    exs[z * 16 + x] = extras.get(&(wi as u32));
                    any |= !cols[wi].out;
                }
            }
            if !any {
                continue;
            }
            let raw = chunk_nbt(&mut pre, (rx * 32 + lx as i64) as i32, (rz * 32 + lz as i64) as i32, &cc, &ovs, &exs, &ex.st.biome_names);
            cbuf.resize(comp.zlib_compress_bound(raw.len()), 0);
            let m = comp.zlib_compress(&raw, &mut cbuf).map_err(|e| anyhow::anyhow!("deflate: {e:?}"))?;
            chunks[lz * 32 + lx] = Some(cbuf[..m].to_vec());
            nchunks += 1;
        }
    }
    if nchunks == 0 {
        return Ok(None);
    }
    Ok(Some((crate::region::build_region(&chunks, 0)?, nchunks)))
}

pub fn unused_fmt(_f: Fmt) {}
