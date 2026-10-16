//! Region generator: terrain columns + land cover + buildings + vegetation
//! (+ roads/rails/structures in infra.rs) → base columns and per-column
//! overlay voxels for the chunk writer.

use crate::blocks::{BlockId, b};
use crate::features::{self, BldRec};
use crate::heightmap::{K_HOLE, K_INLAND, K_LAND1, K_LAND5, K_OUT, K_SEA};
use crate::landcover::Spec;
use std::collections::HashMap;

pub const MIN_Y: i32 = -2032;
pub const SEA_Y: i32 = -1760;

// column flags
pub const CF_BLDG: u16 = 1;
pub const CF_ROAD: u16 = 2;
pub const CF_RAIL: u16 = 4;
pub const CF_STRUCT: u16 = 8;
pub const CF_WATER: u16 = 16;
pub const CF_NO_TREE: u16 = 32; // e.g. under bridges, fields, paddies
pub const CF_PATTERN: u16 = 64; // a land-use pattern occupies the surface
pub const CF_NO_PLANT: u16 = 128;

/// Common block ids (interned once).
pub struct Ids {
    pub air: BlockId,
    pub bedrock: BlockId,
    pub stone: BlockId,
    pub dirt: BlockId,
    pub grass: BlockId,
    pub sand: BlockId,
    pub sandstone: BlockId,
    pub gravel: BlockId,
    pub water: BlockId,
    pub snow: BlockId,
    pub andesite: BlockId,
    pub coarse_dirt: BlockId,
    pub tuff: BlockId,
    pub podzol: BlockId,
    pub mud: BlockId,
    pub farmland: BlockId,
    pub seagrass: BlockId,
    pub glass: BlockId,
    pub smooth_stone: BlockId,
    pub light_gray_concrete: BlockId,
    pub gray_concrete: BlockId,
    pub white_concrete: BlockId,
    pub yellow_concrete: BlockId,
    pub black_concrete: BlockId,
    pub short_grass: BlockId,
    pub tall_grass_lo: BlockId,
    pub tall_grass_hi: BlockId,
    pub fern: BlockId,
    pub large_fern_lo: BlockId,
    pub large_fern_hi: BlockId,
    pub tea_leaves: BlockId,
    pub daylight: BlockId,
    pub grave: BlockId,
    pub crops: [BlockId; 4],
    pub flowers: Vec<BlockId>,
}

impl Ids {
    pub fn get() -> &'static Ids {
        static I: std::sync::OnceLock<Ids> = std::sync::OnceLock::new();
        I.get_or_init(|| Ids {
            air: b("air"),
            bedrock: b("bedrock"),
            stone: b("stone"),
            dirt: b("dirt"),
            grass: b("grass_block[snowy=false]"),
            sand: b("sand"),
            sandstone: b("sandstone"),
            gravel: b("gravel"),
            water: b("water[level=0]"),
            snow: b("snow_block"),
            andesite: b("andesite"),
            coarse_dirt: b("coarse_dirt"),
            tuff: b("tuff"),
            podzol: b("podzol[snowy=false]"),
            mud: b("mud"),
            farmland: b("farmland[moisture=7]"),
            seagrass: b("seagrass"),
            glass: b("glass"),
            smooth_stone: b("smooth_stone"),
            light_gray_concrete: b("light_gray_concrete"),
            gray_concrete: b("gray_concrete"),
            white_concrete: b("white_concrete"),
            yellow_concrete: b("yellow_concrete"),
            black_concrete: b("black_concrete"),
            short_grass: b("short_grass"),
            tall_grass_lo: b("tall_grass[half=lower]"),
            tall_grass_hi: b("tall_grass[half=upper]"),
            fern: b("fern"),
            large_fern_lo: b("large_fern[half=lower]"),
            large_fern_hi: b("large_fern[half=upper]"),
            tea_leaves: b("oak_leaves[distance=7,persistent=true,waterlogged=false]"),
            daylight: b("daylight_detector[inverted=false,power=0]"),
            grave: b("polished_andesite_wall[east=none,north=none,south=none,up=true,waterlogged=false,west=none]"),
            crops: [b("wheat[age=7]"), b("carrots[age=7]"), b("potatoes[age=7]"), b("beetroots[age=3]")],
            flowers: vec![b("poppy"), b("dandelion"), b("azure_bluet"), b("oxeye_daisy"), b("cornflower")],
        })
    }
}

/// Map a spec surface/subsurface name to a block id.
pub fn surface_id(name: &str) -> BlockId {
    let i = Ids::get();
    match name {
        "" | "grass_block" => i.grass,
        "podzol" => i.podzol,
        "farmland[moisture=7]" | "farmland" => i.farmland,
        "dirt" => i.dirt,
        other => b(other),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Col {
    pub out: bool,
    pub g: i32, // top solid block Y
    pub w: i32, // water top Y (== g when no water)
    pub top: BlockId,
    pub sub: BlockId,
    pub sub_n: u8,
    pub deep: BlockId,
    pub deep_n: u8,
    pub biome: u16,
    pub flags: u16,
    pub lc: u16,
}

pub const OUT_COL: Col = Col { out: true, g: MIN_Y - 1, w: MIN_Y - 1, top: 0, sub: 0, sub_n: 0, deep: 0, deep_n: 0, biome: 0, flags: 0, lc: 0 };

impl Col {
    #[inline]
    pub fn block(&self, y: i32, ids: &Ids) -> BlockId {
        if self.out {
            return ids.air;
        }
        if y == MIN_Y {
            return ids.bedrock;
        }
        if y > self.g {
            return if y <= self.w { ids.water } else { ids.air };
        }
        let d = self.g - y;
        if d == 0 {
            self.top
        } else if d <= self.sub_n as i32 {
            self.sub
        } else if d <= (self.sub_n + self.deep_n) as i32 {
            self.deep
        } else {
            ids.stone
        }
    }
    pub fn layer_bottom(&self) -> i32 {
        self.g - (self.sub_n + self.deep_n) as i32
    }
    pub fn top_y(&self) -> i32 {
        self.g.max(self.w)
    }
}

/// Overlay voxels per window column: (y, block). Later puts win.
pub struct Overlay {
    pub v: Vec<Vec<(i32, BlockId)>>,
}

impl Overlay {
    pub fn new(n: usize) -> Self {
        Overlay { v: vec![Vec::new(); n] }
    }
    #[inline]
    pub fn put(&mut self, i: usize, y: i32, blk: BlockId) {
        let c = &mut self.v[i];
        if let Some(e) = c.iter_mut().find(|e| e.0 == y) {
            e.1 = blk;
        } else {
            c.push((y, blk));
        }
    }
    #[inline]
    pub fn get(&self, i: usize, y: i32) -> Option<BlockId> {
        self.v[i].iter().find(|e| e.0 == y).map(|e| e.1)
    }
    /// Put only into empty air above the column surface.
    #[inline]
    pub fn put_air(&mut self, i: usize, y: i32, blk: BlockId, col: &Col) -> bool {
        if col.out || y <= col.top_y() || self.get(i, y).is_some() {
            return false;
        }
        self.v[i].push((y, blk));
        true
    }
    pub fn finish(&mut self) {
        for c in self.v.iter_mut() {
            c.sort_by_key(|e| e.0);
        }
    }
}

#[inline]
pub fn hash3(x: i64, z: i64, s: u64) -> u32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (z as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ s.wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 31;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 29;
    (h >> 32) as u32
}

#[inline]
pub fn unit(x: i64, z: i64, s: u64) -> f32 {
    hash3(x, z, s) as f32 / 4294967296.0
}

/// Specs prepared for fast lookup: index = land-cover class index.
pub struct SpecTable {
    pub specs: Vec<Spec>, // index 0 = none
    pub surface: Vec<BlockId>,
    pub subsurface: Vec<BlockId>,
    pub trees: Vec<Vec<(String, f32)>>,
    pub ground: Vec<Vec<(BlockId, Option<BlockId>, f32)>>, // (lower, upper for double plants, prob)
    pub biome: Vec<u16>,
    pub biome_names: Vec<String>,
    pub height: Vec<(f32, f32)>,
}

impl SpecTable {
    pub fn new(classes: &[Spec]) -> Self {
        let mut biome_names: Vec<String> = vec!["minecraft:the_void".into(), "minecraft:ocean".into(), "minecraft:river".into(), "minecraft:beach".into(), "minecraft:stony_peaks".into(), "minecraft:snowy_slopes".into()];
        let mut bi = |n: &str, names: &mut Vec<String>| -> u16 {
            let n = if n.is_empty() { "minecraft:plains" } else { n };
            match names.iter().position(|x| x == n) {
                Some(p) => p as u16,
                None => {
                    names.push(n.to_string());
                    (names.len() - 1) as u16
                }
            }
        };
        let mut t = SpecTable {
            specs: vec![Spec::default()],
            surface: vec![Ids::get().grass],
            subsurface: vec![Ids::get().dirt],
            trees: vec![vec![]],
            ground: vec![vec![]],
            biome: vec![bi("minecraft:plains", &mut biome_names)],
            biome_names: vec![],
            height: vec![(0.0, 0.0)],
        };
        let ids = Ids::get();
        for s in classes {
            t.surface.push(surface_id(&s.surface));
            t.subsurface.push(surface_id(if s.subsurface.is_empty() { "dirt" } else { &s.subsurface }));
            t.trees.push(s.tree_mix());
            t.ground.push(
                s.ground_mix()
                    .into_iter()
                    .map(|(k, p)| match k.as_str() {
                        "tall_grass" => (ids.tall_grass_lo, Some(ids.tall_grass_hi), p),
                        "large_fern" => (ids.large_fern_lo, Some(ids.large_fern_hi), p),
                        "short_grass" => (ids.short_grass, None, p),
                        "fern" => (ids.fern, None, p),
                        other => (b(other), None, p),
                    })
                    .collect(),
            );
            t.biome.push(bi(&s.biome, &mut biome_names));
            t.height.push(s.height_range());
            t.specs.push(s.clone());
        }
        t.biome_names = biome_names;
        t
    }
}

/// Feature rasters of a region window (same size as the terrain window).
pub struct FeatWin {
    pub lc: Vec<u16>,
    pub bldg: Vec<u32>, // building key: index into `bld` (0 = none)
    pub roof: Vec<f32>,
    pub strct: Vec<u8>,
    pub road: Vec<u8>,
    pub roadh: Vec<f32>,
    pub rail: Vec<u8>,
    pub railh: Vec<f32>,
    pub tcov: Vec<u8>,
    pub lake: Vec<f32>,
    /// field row direction (degrees + 1, 0 = none)
    pub rowdir: Vec<u8>,
    /// LiDAR canopy height of trees (0.2 m units, 0 = none)
    pub canopy: Vec<u8>,
    /// measured trees in the window (absolute X, Z, height m, species code)
    pub trees: Vec<(i64, i64, f32, u8)>,
    pub bld: Vec<BldRec>, // index 0 unused
}

pub struct Gen<'a> {
    pub n: usize,
    pub ox: i64,
    pub oz: i64,
    pub h: &'a [f32],
    pub k: &'a [u8],
    pub fx: &'a FeatWin,
    pub st: &'a SpecTable,
}

/// Euclidean distance (pixels) to the nearest `src` pixel, capped: exact,
/// separable (Felzenszwalb & Huttenlocher), in parallel over column strips
/// and rows.
pub fn distance(src: &[bool], w: usize, h: usize, cap: f32) -> Vec<f32> {
    use rayon::prelude::*;
    const NONE: u32 = u32::MAX;
    // 1) per column: rows to the nearest source pixel in the same column
    let mut g = vec![NONE; w * h];
    {
        struct Ptr(*mut u32);
        unsafe impl Sync for Ptr {}
        let p = Ptr(g.as_mut_ptr());
        const STRIP: usize = 64;
        (0..w.div_ceil(STRIP)).into_par_iter().for_each(|k| {
            let p = &p;
            let (x0, x1) = (k * STRIP, ((k + 1) * STRIP).min(w));
            // SAFETY: every strip writes only its own columns x0..x1
            let at = |i: usize| unsafe { p.0.add(i) };
            for y in 0..h {
                for x in x0..x1 {
                    let i = y * w + x;
                    let v = if src[i] {
                        0
                    } else if y > 0 {
                        unsafe { *at(i - w) }.saturating_add(1)
                    } else {
                        NONE
                    };
                    unsafe { *at(i) = v };
                }
            }
            for y in (0..h.saturating_sub(1)).rev() {
                for x in x0..x1 {
                    let i = y * w + x;
                    let below = unsafe { *at(i + w) }.saturating_add(1);
                    unsafe {
                        if below < *at(i) {
                            *at(i) = below;
                        }
                    }
                }
            }
        });
    }
    // 2) per row: lower envelope of the parabolas (x - q)² + g(q)²
    const FAR: f64 = 1e12;
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each_init(
        || (vec![0f64; w], vec![0usize; w], vec![0f64; w + 1]),
        |(f, v, z), (y, row)| {
            for x in 0..w {
                let gv = g[y * w + x];
                f[x] = if gv == NONE { FAR } else { (gv as f64) * (gv as f64) };
            }
            let isect = |f: &[f64], q: usize, r: usize| ((f[q] + (q * q) as f64) - (f[r] + (r * r) as f64)) / (2.0 * (q as f64 - r as f64));
            let mut k = 0usize;
            v[0] = 0;
            z[0] = f64::NEG_INFINITY;
            z[1] = f64::INFINITY;
            for q in 1..w {
                let mut s = isect(f, q, v[k]);
                while s <= z[k] {
                    k -= 1;
                    s = isect(f, q, v[k]);
                }
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = f64::INFINITY;
            }
            k = 0;
            for (q, o) in row.iter_mut().enumerate() {
                while z[k + 1] < q as f64 {
                    k += 1;
                }
                let dq = q as f64 - v[k] as f64;
                *o = ((dq * dq + f[v[k]]).sqrt() as f32).min(cap);
            }
        },
    );
    out
}

impl<'a> Gen<'a> {
    #[inline]
    fn wx(&self, i: usize) -> i64 {
        self.ox + (i % self.n) as i64
    }
    #[inline]
    fn wz(&self, i: usize) -> i64 {
        self.oz + (i / self.n) as i64
    }

    /// Terrain columns with land-cover surfaces (no structures yet).
    pub fn columns(&self) -> Vec<Col> {
        let ids = Ids::get();
        let n = self.n;
        let (h, k) = (self.h, self.k);
        let is_land = |c: u8| c == K_LAND1 || c == K_LAND5 || c == K_HOLE;
        let sea: Vec<bool> = k.iter().map(|&c| c == K_SEA).collect();
        let dsea = distance(&sea, n, n, 64.0);
        // distance from the banks (land, not the sea at a river mouth) for the shelving river bed
        let bank: Vec<bool> = k.iter().map(|&c| c != K_INLAND && c != K_SEA).collect();
        let dshore = distance(&bank, n, n, 32.0);
        let inl: Vec<bool> = k.iter().map(|&c| c == K_INLAND).collect();
        let dinl = distance(&inl, n, n, 8.0);
        let mut g = vec![MIN_Y - 1; n * n];
        let mut wtop = vec![MIN_Y - 1; n * n];
        for i in 0..n * n {
            match k[i] {
                K_OUT => {}
                K_SEA => {
                    // sea floor from the depth layer (ETOPO 2022); what lies deeper
                    // than the world can hold is cut off
                    let d = self.fx.lake[i];
                    let depth = if d.is_finite() { d.round().clamp(1.0, (SEA_Y - MIN_Y - 4) as f32) as i32 } else { 250 };
                    g[i] = SEA_Y - depth;
                    wtop[i] = SEA_Y;
                }
                K_INLAND => {
                    let ws = SEA_Y + h[i].round() as i32;
                    // surveyed lake bed where available, else a shelving bottom
                    let lake = self.fx.lake[i];
                    let depth = if lake.is_finite() { (lake.round() as i32).max(1) } else { (1.0 + dshore[i] / 3.0).min(8.0) as i32 };
                    g[i] = (ws - depth).max(MIN_Y + 1);
                    wtop[i] = ws;
                }
                _ => {
                    g[i] = SEA_Y + h[i].round() as i32;
                    wtop[i] = g[i];
                }
            }
        }
        // land next to water is at least as high as the water surface
        let mut g2 = g.clone();
        for y in 0..n {
            for x in 0..n {
                let i = y * n + x;
                if !is_land(k[i]) {
                    continue;
                }
                let mut m = g[i];
                for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                    let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                    if xx < 0 || yy < 0 || xx >= n as i64 || yy >= n as i64 {
                        continue;
                    }
                    let j = yy as usize * n + xx as usize;
                    if k[j] == K_SEA || k[j] == K_INLAND {
                        m = m.max(wtop[j]);
                    }
                }
                g2[i] = m;
            }
        }
        let g = g2;
        // share of building pixels within ±15 m: open ground in dense built-up
        // areas is paved (asphalt, concrete), not lawn
        let mut integ = vec![0u32; (n + 1) * (n + 1)];
        for y in 0..n {
            let mut row = 0u32;
            for x in 0..n {
                row += (self.fx.bldg[y * n + x] != 0) as u32;
                integ[(y + 1) * (n + 1) + x + 1] = integ[y * (n + 1) + x + 1] + row;
            }
        }
        let bfrac = |x: usize, y: usize| -> f32 {
            let (x0, y0, x1, y1) = (x.saturating_sub(15), y.saturating_sub(15), (x + 16).min(n), (y + 16).min(n));
            let s = integ[y1 * (n + 1) + x1] + integ[y0 * (n + 1) + x0] - integ[y0 * (n + 1) + x1] - integ[y1 * (n + 1) + x0];
            s as f32 / ((x1 - x0) * (y1 - y0)) as f32
        };
        let mut cols = vec![OUT_COL; n * n];
        for y in 0..n {
            for x in 0..n {
                let i = y * n + x;
                if k[i] == K_OUT {
                    continue;
                }
                let (wx, wz) = (self.wx(i), self.wz(i));
                let lc = self.fx.lc[i];
                let mut c = Col { out: false, g: g[i], w: wtop[i].max(g[i]), top: ids.stone, sub: ids.stone, sub_n: 0, deep: ids.stone, deep_n: 0, biome: 0, flags: 0, lc };
                match k[i] {
                    K_SEA => {
                        c.top = ids.sand;
                        c.sub = ids.sand;
                        c.sub_n = 3;
                        c.deep = ids.sandstone;
                        c.deep_n = 3;
                        c.biome = 1;
                        c.flags |= CF_WATER;
                    }
                    K_INLAND => {
                        c.top = if dshore[i] > 4.0 { ids.sand } else { ids.gravel };
                        c.sub = ids.dirt;
                        c.sub_n = 2;
                        c.biome = 2;
                        c.flags |= CF_WATER;
                    }
                    _ => {
                        let mut s = 0;
                        for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                            let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                            if xx < 0 || yy < 0 || xx >= n as i64 || yy >= n as i64 {
                                continue;
                            }
                            let j = yy as usize * n + xx as usize;
                            if is_land(k[j]) {
                                s = s.max((g[j] - g[i]).abs());
                            }
                        }
                        let e = h[i];
                        let patch = crate::world::vnoise(wx, wz, 6, 7);
                        let snow_line = 2950.0 + 120.0 * crate::world::vnoise(wx, wz, 40, 3);
                        let alpine_line = 2450.0 + 100.0 * crate::world::vnoise(wx, wz, 30, 5);
                        let r = hash3(wx, wz, 1) % 100;
                        let cat = self.st.specs[lc as usize].category.as_str();
                        if e >= snow_line && s <= 2 {
                            c.top = ids.snow;
                            c.sub = ids.snow;
                            c.sub_n = 1;
                            c.biome = 5;
                        } else if e >= alpine_line && cat != "alpine" {
                            c.top = if s >= 3 { ids.stone } else if patch < 0.30 { ids.gravel } else if patch < 0.50 { ids.tuff } else if patch < 0.75 { ids.andesite } else { ids.stone };
                            c.sub = if c.top == ids.gravel { ids.gravel } else { ids.stone };
                            c.sub_n = 2;
                            c.biome = if e >= snow_line { 5 } else { 4 };
                            c.flags |= CF_NO_TREE;
                        } else if s >= 3 || (s == 2 && r < 60 && !matches!(cat, "urban" | "residential" | "industrial" | "farm" | "paddy" | "tea")) {
                            c.top = if patch < 0.3 { ids.andesite } else { ids.stone };
                            c.biome = self.st.biome[lc as usize];
                            c.flags |= CF_NO_PLANT;
                        } else if e <= 4.0 && dsea[i] <= 40.0 && s <= 1 && matches!(cat, "grass" | "bare" | "sand" | "" | "water" | "wetland") {
                            c.top = ids.sand;
                            c.sub = ids.sand;
                            c.sub_n = 3;
                            c.deep = ids.sandstone;
                            c.deep_n = 3;
                            c.biome = 3;
                            c.flags |= CF_NO_TREE;
                        } else if dinl[i] <= 2.0 && s <= 1 && matches!(cat, "grass" | "bare" | "" | "water" | "wetland" | "shrub") {
                            c.top = ids.gravel;
                            c.sub = ids.gravel;
                            c.sub_n = 1;
                            c.deep = ids.dirt;
                            c.deep_n = 2;
                            c.biome = 2;
                        } else if matches!(cat, "urban" | "industrial" | "port" | "residential") && {
                            // share of paved ground grows with the building density
                            let (lo, span) = if cat == "residential" { (0.20, 0.25) } else { (0.08, 0.20) };
                            let prob = ((bfrac(x, y) - lo) / span).clamp(0.0, 1.0);
                            crate::world::vnoise(wx, wz, 5, 29) < prob
                        } {
                            let v = crate::world::vnoise(wx, wz, 8, 17);
                            c.top = if v < 0.55 { ids.gray_concrete } else if v < 0.8 { ids.light_gray_concrete } else { ids.andesite };
                            c.sub = ids.stone;
                            c.sub_n = 2;
                            c.biome = self.st.biome[lc as usize];
                            // paved: no lawn plants or planted grid trees (measured street trees stay)
                            c.flags |= CF_PATTERN | CF_NO_TREE | CF_NO_PLANT;
                        } else {
                            c.top = self.st.surface[lc as usize];
                            c.sub = self.st.subsurface[lc as usize];
                            c.sub_n = 3;
                            c.biome = self.st.biome[lc as usize];
                            if e >= alpine_line - 50.0 {
                                c.biome = 4;
                            }
                        }
                    }
                }
                cols[i] = c;
            }
        }
        cols
    }
}

// ---------------------------------------------------------------- land-use patterns

pub fn patterns(rg: &Gen, cols: &mut [Col], ov: &mut Overlay) {
    let ids = Ids::get();
    let n = rg.n;
    // surface tops before any pattern changed them (paddies lower their water)
    let top0: Vec<i32> = cols.iter().map(|c| if c.out { i32::MIN } else { c.top_y() }).collect();
    for i in 0..n * n {
        let c = cols[i];
        if c.out || c.flags & (CF_WATER | CF_BLDG | CF_ROAD | CF_RAIL | CF_STRUCT) != 0 {
            continue;
        }
        let lc = c.lc as usize;
        let sp = &rg.st.specs[lc];
        if sp.pattern.is_empty() || sp.pattern == "none" {
            continue;
        }
        if c.top == ids.stone || c.top == ids.andesite || c.top == ids.snow {
            continue; // steep / alpine parts keep their rock
        }
        let (x, z) = (rg.wx(i), rg.wz(i));
        let same = |j: usize| -> bool { cols[j].lc == c.lc && cols[j].flags & (CF_BLDG | CF_ROAD | CF_RAIL) == 0 };
        let nb = |dx: i64, dz: i64| -> Option<usize> {
            let (xx, zz) = ((i % n) as i64 + dx, (i / n) as i64 + dz);
            (xx >= 0 && zz >= 0 && xx < n as i64 && zz < n as i64).then(|| zz as usize * n + xx as usize)
        };
        let boundary = [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().any(|&(dx, dz)| nb(dx, dz).map(|j| !same(j)).unwrap_or(true));
        // rows run along the nearest road; without one, per 64 m cell along X or Z
        let rd = rg.fx.rowdir[i];
        let row = if rd > 0 {
            let th = ((rd - 1) as f64).to_radians();
            (-(x as f64) * th.sin() + (z as f64) * th.cos()).floor() as i64
        } else if hash3(x.div_euclid(64), z.div_euclid(64), 11) & 1 == 0 {
            z
        } else {
            x
        };
        let mut cm = cols[i];
        match sp.pattern.as_str() {
            "paddy" => {
                // flooded paddy: one block of water over mud, rows of rice
                // (seagrass), held in by the levees (field edges stay mud at
                // ground level) and by neighbours at least as high
                let contained = [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().all(|&(dx, dz)| nb(dx, dz).map(|j| top0[j] >= c.g).unwrap_or(false));
                cm.sub = ids.mud;
                cm.sub_n = 2;
                cm.top = ids.mud;
                if contained && !boundary {
                    cm.g = c.g - 1;
                    cm.w = c.g;
                    if row.rem_euclid(2) == 0 {
                        ov.put(i, c.g, ids.seagrass);
                    }
                }
                cm.flags |= CF_PATTERN | CF_NO_TREE | CF_NO_PLANT;
            }
            "field" => {
                cm.top = ids.farmland;
                cm.sub = ids.dirt;
                cm.flags |= CF_PATTERN | CF_NO_TREE;
                if row.rem_euclid(5) != 4 && !boundary {
                    let crop = ids.crops[(hash3(x.div_euclid(24), z.div_euclid(24), 5) % 4) as usize];
                    ov.put_air(i, c.g + 1, crop, &cm);
                } else {
                    cm.top = ids.coarse_dirt;
                }
            }
            "tea_rows" => {
                cm.top = ids.grass;
                cm.flags |= CF_PATTERN | CF_NO_TREE | CF_NO_PLANT;
                if row.rem_euclid(2) == 0 && !boundary {
                    ov.put_air(i, c.g + 1, ids.tea_leaves, &cm);
                } else {
                    cm.top = ids.coarse_dirt;
                }
            }
            "greenhouse" => {
                // long glass houses (7 wide + 1 gap), 3 high, glass roof
                let lane = row.rem_euclid(8);
                cm.top = if lane == 7 { ids.gravel } else { ids.farmland };
                cm.flags |= CF_PATTERN | CF_NO_TREE | CF_NO_PLANT;
                if lane != 7 {
                    let wall = boundary || lane == 0 || lane == 6;
                    for dy in 1..=3 {
                        if wall {
                            ov.put_air(i, c.g + dy, ids.glass, &cm);
                        }
                    }
                    ov.put_air(i, c.g + 4, b("white_stained_glass"), &cm);
                }
            }
            "solar" => {
                cm.top = ids.gravel;
                cm.flags |= CF_PATTERN | CF_NO_TREE | CF_NO_PLANT;
                if row.rem_euclid(3) != 2 {
                    ov.put_air(i, c.g + 1, ids.daylight, &cm);
                }
            }
            "cemetery" => {
                cm.top = ids.gravel;
                cm.flags |= CF_PATTERN | CF_NO_TREE | CF_NO_PLANT;
                if x.rem_euclid(2) == 0 && z.rem_euclid(3) == 0 && !boundary {
                    ov.put_air(i, c.g + 1, ids.grave, &cm);
                }
            }
            "parking" => {
                cm.top = if row.rem_euclid(3) == 0 && !boundary { ids.light_gray_concrete } else { ids.gray_concrete };
                cm.sub = ids.stone;
                cm.flags |= CF_PATTERN | CF_NO_TREE | CF_NO_PLANT;
            }
            "runway" => {
                cm.top = ids.black_concrete;
                cm.sub = ids.stone;
                cm.flags |= CF_PATTERN | CF_NO_TREE | CF_NO_PLANT;
            }
            "apron" => {
                cm.top = gen_surface(sp, ids);
                cm.sub = ids.stone;
                cm.flags |= CF_PATTERN | CF_NO_TREE | CF_NO_PLANT;
            }
            "sports" => {
                cm.top = if boundary { ids.white_concrete } else { ids.grass };
                cm.flags |= CF_PATTERN | CF_NO_TREE | CF_NO_PLANT;
            }
            "golf" => {
                let bunker = crate::world::vnoise(x, z, 9, 21) > 0.82;
                cm.top = if bunker { ids.sand } else { ids.grass };
                cm.flags |= CF_PATTERN | CF_NO_PLANT;
            }
            "orchard" => {
                cm.top = ids.grass;
                cm.flags |= CF_PATTERN;
            }
            _ => {}
        }
        cols[i] = cm;
    }
}

fn gen_surface(sp: &Spec, ids: &Ids) -> BlockId {
    if sp.surface.is_empty() { ids.light_gray_concrete } else { surface_id(&sp.surface) }
}

// ---------------------------------------------------------------- trees

fn leaves(kind: &str) -> BlockId {
    b(&format!("{kind}_leaves[distance=7,persistent=true,waterlogged=false]"))
}
fn log(kind: &str) -> BlockId {
    b(&format!("{kind}_log[axis=y]"))
}

/// Voxels of one tree relative to its base (dx, dy, dz, block); dy = 0 is the
/// first block above the ground.
pub fn tree_voxels(species: &str, h: i32, x: i64, z: i64, out: &mut Vec<(i32, i32, i32, BlockId)>) {
    out.clear();
    let h = h.max(1);
    let rnd = |s: u64| unit(x, z, s);
    let mut trunk = |kind: &str, top: i32, out: &mut Vec<(i32, i32, i32, BlockId)>| {
        let l = log(kind);
        for y in 0..top {
            out.push((0, y, 0, l));
        }
    };
    let disc = |y: i32, r: f32, blk: BlockId, fill: f32, s: u64, out: &mut Vec<(i32, i32, i32, BlockId)>| {
        let ri = r.ceil() as i32;
        for dz in -ri..=ri {
            for dx in -ri..=ri {
                let d2 = (dx * dx + dz * dz) as f32;
                if d2 <= r * r + 0.3 && (fill >= 1.0 || unit(x + dx as i64 * 7, z + dz as i64 * 13 + y as i64 * 31, s) < fill || d2 < 1.0) {
                    out.push((dx, y, dz, blk));
                }
            }
        }
    };
    let ellipsoid = |cy: f32, rx: f32, ry: f32, blk: BlockId, fill: f32, out: &mut Vec<(i32, i32, i32, BlockId)>| {
        let (ri, rj) = (rx.ceil() as i32, ry.ceil() as i32);
        for dy in -rj..=rj {
            for dz in -ri..=ri {
                for dx in -ri..=ri {
                    let e = (dx * dx + dz * dz) as f32 / (rx * rx) + (dy * dy) as f32 / (ry * ry);
                    if e <= 1.0 && (fill >= 1.0 || e < 0.5 || unit(x + dx as i64 * 7, z + dz as i64 * 13 + dy as i64 * 31, 77) < fill) {
                        out.push((dx, (cy + dy as f32).round() as i32, dz, blk));
                    }
                }
            }
        }
    };
    match species {
        "cedar" | "cypress" | "fir" | "spruce" | "larch" => {
            let (rmax, leaf, bark, start, fill) = match species {
                "cedar" => ((h as f32 * 0.11).clamp(1.2, 3.0), leaves("spruce"), "spruce", 0.30, 0.95),
                "cypress" => ((h as f32 * 0.13).clamp(1.4, 3.4), leaves("spruce"), "spruce", 0.35, 0.95),
                "fir" => ((h as f32 * 0.16).clamp(1.5, 4.0), leaves("spruce"), "spruce", 0.25, 0.9),
                "spruce" => ((h as f32 * 0.17).clamp(1.5, 4.5), leaves("spruce"), "spruce", 0.25, 0.9),
                _ => ((h as f32 * 0.15).clamp(1.5, 3.8), leaves("birch"), "spruce", 0.35, 0.7),
            };
            trunk(bark, h - 1, out);
            let y0 = (h as f32 * start) as i32;
            for y in y0..=h {
                let t = (y - y0) as f32 / (h - y0).max(1) as f32;
                let mut r = rmax * (1.0 - t) + 0.4;
                if species == "fir" || species == "spruce" {
                    // layered silhouette
                    if (y - y0) % 2 == 1 {
                        r *= 0.75;
                    }
                }
                disc(y, r, leaf, fill, 3, out);
            }
            out.push((0, h + 1, 0, leaf));
        }
        "pine" => {
            trunk("jungle", h - 1, out);
            let leaf = leaves("spruce");
            let r = (h as f32 * 0.22).clamp(2.0, 4.5);
            ellipsoid(h as f32 * 0.85, r, (h as f32 * 0.12).max(1.5), leaf, 0.75, out);
            // a lower side cluster
            let side = if rnd(9) < 0.5 { 1 } else { -1 };
            for dy in 0..2 {
                disc((h as f32 * 0.65) as i32 + dy, 1.6, leaf, 0.8, 4, out);
            }
            let _ = side;
        }
        "broadleaf" | "beech" | "willow" | "cherry" | "evergreen_broadleaf" | "fruit" | "citrus" => {
            let (bark, leaf, rfac, crown_c) = match species {
                "beech" => ("birch", leaves("oak"), 0.30, 0.68),
                "willow" => ("oak", leaves("oak"), 0.35, 0.62),
                "cherry" => ("cherry", leaves("cherry"), 0.45, 0.65),
                "evergreen_broadleaf" => ("dark_oak", leaves("dark_oak"), 0.36, 0.66),
                "fruit" => ("oak", leaves("oak"), 0.55, 0.65),
                "citrus" => ("dark_oak", leaves("jungle"), 0.5, 0.6),
                _ => ("oak", leaves("oak"), 0.27, 0.68),
            };
            let cy = h as f32 * crown_c;
            trunk(bark, (cy as i32).max(1), out);
            let rx = (h as f32 * rfac).clamp(1.5, 6.0);
            let ry = (h as f32 * (1.0 - crown_c) + 0.5).max(1.5);
            ellipsoid(cy, rx, ry, leaf, 0.85, out);
            if species == "willow" {
                // hanging fringe
                for a in 0..12 {
                    let ang = a as f32 / 12.0 * std::f32::consts::TAU;
                    let (dx, dz) = ((rx * ang.cos()).round() as i32, (rx * ang.sin()).round() as i32);
                    for dy in 1..=3 {
                        out.push((dx, cy as i32 - dy, dz, leaf));
                    }
                }
            }
        }
        "birch" => {
            trunk("birch", h - 1, out);
            ellipsoid(h as f32 * 0.7, (h as f32 * 0.18).clamp(1.5, 3.5), h as f32 * 0.3, leaves("birch"), 0.85, out);
        }
        "bamboo" => {
            // stage 1: grown, so the stalks do not keep growing on random ticks
            let lg = b("bamboo[age=1,leaves=large,stage=1]");
            let sm = b("bamboo[age=1,leaves=small,stage=1]");
            let no = b("bamboo[age=1,leaves=none,stage=1]");
            for y in 0..h {
                let blk = if y >= h - 2 { lg } else if y >= h - 4 { sm } else { no };
                out.push((0, y, 0, blk));
            }
        }
        "dwarf_pine" => {
            let leaf = leaves("spruce");
            let r = 1.0 + rnd(5) * 1.2;
            disc(0, r, leaf, 0.9, 6, out);
            if h >= 2 {
                disc(1, r * 0.6, leaf, 0.8, 7, out);
            }
        }
        "shrub" => {
            let leaf = leaves(if rnd(8) < 0.5 { "oak" } else { "azalea" });
            disc(0, 1.2, leaf, 0.8, 8, out);
            if h >= 2 {
                out.push((0, 1, 0, leaf));
            }
        }
        _ => {
            trunk("oak", (h as f32 * 0.6) as i32, out);
            ellipsoid(h as f32 * 0.7, 2.5, 2.0, leaves("oak"), 0.85, out);
        }
    }
}

/// Species from a class's tree mix, chosen per position.
fn pick_species(st: &SpecTable, lc: u16, x: i64, z: i64) -> Option<&str> {
    let mix = &st.trees[lc as usize];
    if mix.is_empty() {
        return None;
    }
    let total: f32 = mix.iter().map(|m| m.1).sum();
    let mut r = unit(x, z, 43) * total;
    for (s, w) in mix {
        if r < *w {
            return Some(s.as_str());
        }
        r -= w;
    }
    Some(mix[0].0.as_str())
}

/// Voxelize one tree with its trunk at window position (lx, lz).
fn place_tree(rg: &Gen, cols: &[Col], ov: &mut Overlay, vox: &mut Vec<(i32, i32, i32, BlockId)>, species: &str, mut ht: f32, x: i64, z: i64) {
    let n = rg.n as i64;
    let (lx, lz) = (x - rg.ox, z - rg.oz);
    let c = cols[(lz * n + lx) as usize];
    // tree line: no tall trees above ~2,500 m, krummholz just below it
    let elev = (c.g - SEA_Y) as f32;
    if elev > 2400.0 && !matches!(species, "dwarf_pine" | "shrub") {
        if elev > 2550.0 {
            return;
        }
        ht = ht.min(3.0 + (2550.0 - elev) / 15.0);
    }
    let species = if ht < 2.5 && !matches!(species, "dwarf_pine" | "bamboo") { "shrub" } else { species };
    // nothing may stand in the trunk's way (bridge decks, canopies, wires)
    let i = (lz * n + lx) as usize;
    let top = c.g + ht.round() as i32 + 2;
    if ov.v[i].iter().any(|&(y, _)| y > c.g && y <= top) {
        return;
    }
    tree_voxels(species, ht.round() as i32, x, z, vox);
    let base = c.g + 1;
    for &(dx, dy, dz, blk) in vox.iter() {
        let (px, pz) = (lx + dx as i64, lz + dz as i64);
        if px < 0 || pz < 0 || px >= n || pz >= n {
            continue;
        }
        let j = (pz * n + px) as usize;
        let cj = &cols[j];
        if cj.out || cj.flags & (CF_WATER | CF_BLDG | CF_ROAD | CF_RAIL | CF_STRUCT) != 0 {
            continue;
        }
        let y = base + dy;
        if (dx != 0 || dz != 0) && ov.get(j, y).is_some() {
            continue; // leaves never replace other blocks
        }
        if dx == 0 && dz == 0 {
            if y <= cj.top_y() {
                continue;
            }
            ov.put(j, y, blk);
        } else {
            ov.put_air(j, y, blk, cj);
        }
    }
}

/// Plant trees. Measured trees (Shizuoka: forest-analysis tree points and
/// tree tops of the LiDAR canopy model) are placed where they stand with
/// their height; elsewhere every land-cover class with trees gets them on a
/// global jittered grid, so neighbouring regions agree on every tree.
pub fn trees(rg: &Gen, cols: &[Col], ov: &mut Overlay) {
    let n = rg.n as i64;
    let mut vox = Vec::new();
    for &(x, z, h, sp) in &rg.fx.trees {
        let (lx, lz) = (x - rg.ox, z - rg.oz);
        if lx < 0 || lz < 0 || lx >= n || lz >= n {
            continue;
        }
        let i = (lz * n + lx) as usize;
        let c = cols[i];
        if c.out || c.flags & (CF_WATER | CF_BLDG | CF_ROAD | CF_RAIL | CF_STRUCT) != 0 {
            continue;
        }
        // not under bridges or on alpine rock (fields and other patterns may have trees)
        if c.flags & CF_NO_TREE != 0 && c.flags & CF_PATTERN == 0 {
            continue;
        }
        let elev = (c.g - SEA_Y) as f32;
        let species = crate::trees::species_name(sp, elev, x, z)
            .or_else(|| pick_species(rg.st, c.lc, x, z))
            .unwrap_or(if elev < 500.0 && unit(x, z, 63) < 0.6 { "evergreen_broadleaf" } else { "broadleaf" });
        place_tree(rg, cols, ov, &mut vox, species, h.clamp(1.0, 50.0), x, z);
    }
    // close the measured canopy: where the LiDAR canopy surface has no crown
    // of a placed tree, add the top two layers of leaves at its height
    let (needle, broad) = (leaves("spruce"), leaves("oak"));
    for i in 0..rg.n * rg.n {
        let ch = rg.fx.canopy[i];
        if ch == 0 {
            continue;
        }
        let c = &cols[i];
        if c.out || c.flags & (CF_WATER | CF_BLDG | CF_ROAD | CF_RAIL | CF_STRUCT) != 0 {
            continue;
        }
        let top = c.g + (ch as f32 / 5.0).round() as i32;
        if ov.v[i].iter().any(|&(y, _)| y >= top - 2 && y <= top + 1) {
            continue; // a crown is already there
        }
        let (x, z) = (rg.wx(i), rg.wz(i));
        let conifer = pick_species(rg.st, c.lc, x, z).is_some_and(|s| matches!(s, "cedar" | "cypress" | "fir" | "spruce" | "larch" | "pine"));
        let leaf = if conifer { needle } else { broad };
        ov.put_air(i, top, leaf, c);
        ov.put_air(i, top - 1, leaf, c);
    }
    // cell sizes used by the classes present
    let mut by_cell: std::collections::BTreeMap<i64, Vec<u16>> = std::collections::BTreeMap::new();
    let mut present = vec![false; rg.st.specs.len()];
    for c in cols {
        if !c.out {
            present[c.lc as usize] = true;
        }
    }
    for (lc, p) in present.iter().enumerate() {
        if !*p || rg.st.trees[lc].is_empty() {
            continue;
        }
        let sp = &rg.st.specs[lc];
        let cell = if sp.pattern == "orchard" { 5 } else if sp.density > 0.0 { (10.0 / sp.density.sqrt()).round().clamp(1.0, 40.0) as i64 } else { continue };
        by_cell.entry(cell).or_default().push(lc as u16);
    }
    for (cell, classes) in by_cell {
        let (gx0, gz0) = (rg.ox.div_euclid(cell), rg.oz.div_euclid(cell));
        let (gx1, gz1) = ((rg.ox + n - 1).div_euclid(cell), (rg.oz + n - 1).div_euclid(cell));
        let orchard_grid = classes.iter().any(|&l| rg.st.specs[l as usize].pattern == "orchard");
        for gz in gz0..=gz1 {
            for gx in gx0..=gx1 {
                let (jx, jz) = if orchard_grid && cell == 5 { (2, 2) } else { ((unit(gx, gz, 31) * cell as f32) as i64, (unit(gx, gz, 37) * cell as f32) as i64) };
                let (x, z) = (gx * cell + jx, gz * cell + jz);
                let (lx, lz) = (x - rg.ox, z - rg.oz);
                if lx < 0 || lz < 0 || lx >= n || lz >= n {
                    continue;
                }
                let i = (lz * n + lx) as usize;
                let c = cols[i];
                if c.out || !classes.contains(&c.lc) {
                    continue;
                }
                if c.flags & (CF_WATER | CF_BLDG | CF_ROAD | CF_RAIL | CF_STRUCT | CF_NO_TREE) != 0 {
                    continue;
                }
                // measured: planted forest tree by tree, or the LiDAR canopy
                if rg.fx.tcov[i] != 0 {
                    continue;
                }
                let sp = &rg.st.specs[c.lc as usize];
                if sp.pattern != "orchard" && unit(x, z, 41) > 0.85 {
                    continue; // natural gaps
                }
                let Some(species) = pick_species(rg.st, c.lc, x, z) else { continue };
                let (hmin, hmax) = rg.st.height[c.lc as usize];
                let ht = hmin + (hmax - hmin) * unit(x, z, 47);
                place_tree(rg, cols, ov, &mut vox, species, ht, x, z);
            }
        }
    }
}

/// Grass, ferns and flowers on suitable ground.
pub fn plants(rg: &Gen, cols: &[Col], ov: &mut Overlay) {
    let ids = Ids::get();
    for i in 0..rg.n * rg.n {
        let c = &cols[i];
        if c.out || c.flags & (CF_WATER | CF_BLDG | CF_ROAD | CF_RAIL | CF_STRUCT | CF_NO_PLANT | CF_PATTERN) != 0 {
            continue;
        }
        if !(c.top == ids.grass || c.top == ids.podzol || c.top == ids.coarse_dirt || c.top == ids.dirt) {
            continue;
        }
        let g = &rg.st.ground[c.lc as usize];
        if g.is_empty() {
            continue;
        }
        let (x, z) = (rg.wx(i), rg.wz(i));
        let mut r = unit(x, z, 53);
        for &(lo, hi, p) in g {
            if r < p {
                if ov.get(i, c.g + 1).is_none() && (hi.is_none() || ov.get(i, c.g + 2).is_none()) {
                    ov.put_air(i, c.g + 1, lo, c);
                    if let Some(hb) = hi {
                        ov.put_air(i, c.g + 2, hb, c);
                    }
                }
                break;
            }
            r -= p;
        }
    }
}

// ---------------------------------------------------------------- buildings

struct Mats {
    wall: BlockId,
    window: BlockId,
    roof: BlockId,
    floor: BlockId,
    frame: BlockId,
}

fn materials(r: &BldRec) -> Mats {
    let pick = |opts: &[&str], s: u32| b(opts[(s as usize) % opts.len()]);
    let s = r.seed;
    let glass = b("glass");
    match r.usage {
        features::U_HOUSE => Mats {
            wall: pick(&["white_terracotta", "light_gray_terracotta", "birch_planks", "smooth_sandstone", "white_concrete", "brown_terracotta"], s),
            window: glass,
            roof: pick(&["deepslate_tiles", "deepslate_tiles", "dark_prismarine", "bricks", "polished_blackstone", "brown_concrete"], s >> 8),
            floor: b("oak_planks"),
            frame: b("spruce_planks"),
        },
        features::U_APARTMENT | features::U_HOTEL => Mats {
            wall: pick(&["white_concrete", "light_gray_concrete", "smooth_quartz", "white_terracotta", "smooth_sandstone"], s),
            window: glass,
            roof: b("smooth_stone"),
            floor: b("polished_andesite"),
            frame: b("light_gray_concrete"),
        },
        features::U_COMMERCIAL | features::U_STATION => Mats {
            wall: pick(&["light_blue_stained_glass", "glass", "light_gray_concrete", "white_concrete"], s),
            window: pick(&["light_blue_stained_glass", "glass"], s >> 4),
            roof: b("smooth_stone"),
            floor: b("polished_andesite"),
            frame: b("gray_concrete"),
        },
        features::U_OFFICE => Mats {
            wall: pick(&["gray_concrete", "light_gray_concrete", "cyan_terracotta"], s),
            window: pick(&["light_blue_stained_glass", "glass", "gray_stained_glass"], s >> 4),
            roof: b("smooth_stone"),
            floor: b("polished_andesite"),
            frame: b("gray_concrete"),
        },
        features::U_FACTORY | features::U_WAREHOUSE => Mats {
            wall: pick(&["light_gray_concrete", "white_concrete", "cyan_terracotta", "light_blue_terracotta", "iron_block"], s),
            window: glass,
            roof: pick(&["light_gray_concrete", "smooth_stone", "cyan_concrete", "gray_concrete"], s >> 8),
            floor: b("smooth_stone"),
            frame: b("gray_concrete"),
        },
        features::U_PUBLIC | features::U_SCHOOL | features::U_HOSPITAL => Mats {
            wall: pick(&["white_concrete", "smooth_sandstone", "light_gray_concrete", "white_terracotta"], s),
            window: glass,
            roof: b("smooth_stone"),
            floor: b("polished_andesite"),
            frame: b("light_gray_concrete"),
        },
        features::U_RELIGIOUS => Mats {
            wall: pick(&["dark_oak_planks", "spruce_planks", "stripped_dark_oak_log[axis=y]"], s),
            window: b("spruce_planks"),
            roof: b("deepslate_tiles"),
            floor: b("spruce_planks"),
            frame: b("dark_oak_log[axis=y]"),
        },
        features::U_GREENHOUSE => Mats { wall: glass, window: glass, roof: b("white_stained_glass"), floor: b("farmland[moisture=7]"), frame: glass },
        features::U_SHED => Mats {
            wall: pick(&["spruce_planks", "light_gray_concrete", "iron_block", "oak_planks"], s),
            window: pick(&["spruce_planks", "light_gray_concrete"], s),
            roof: pick(&["smooth_stone", "light_gray_concrete", "deepslate_tiles", "cyan_concrete"], s >> 8),
            floor: b("smooth_stone"),
            frame: b("spruce_log[axis=y]"),
        },
        _ => Mats { wall: b("light_gray_concrete"), window: glass, roof: b("smooth_stone"), floor: b("polished_andesite"), frame: b("gray_concrete") },
    }
}

/// Voxelize all buildings in the window. `decks` are the bridge decks over
/// each column (see infra::apply): a building mostly under a viaduct stays
/// below it; one mostly over the railway is a station roof.
pub fn buildings(rg: &Gen, cols: &mut [Col], ov: &mut Overlay, decks: &crate::infra::Decks) {
    let deck_bottom = &decks.bottom;
    let n = rg.n;
    let fx = rg.fx;
    // footprint pixels per building (water never), and how much of it lies on
    // the railway (on the ground or on a viaduct) and under any deck
    let mut pix: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut on_rail: HashMap<u32, u32> = HashMap::new();
    let mut under_deck: HashMap<u32, u32> = HashMap::new();
    for i in 0..n * n {
        let k = fx.bldg[i];
        if k == 0 || cols[i].out || cols[i].flags & CF_WATER != 0 {
            continue;
        }
        pix.entry(k).or_default().push(i);
        if fx.rail[i] != 0 || decks.rail[i] || cols[i].flags & CF_RAIL != 0 {
            *on_rail.entry(k).or_default() += 1;
        }
        if deck_bottom[i] != i32::MAX {
            *under_deck.entry(k).or_default() += 1;
        }
    }
    let ids = Ids::get();
    let nn = n as i64;
    let mut keys: Vec<u32> = pix.keys().copied().collect();
    keys.sort();
    for key in keys {
        let all = &pix[&key];
        let r = fx.bld[key as usize];
        // a footprint mostly over the railway is a station roof: 基盤地図情報
        // sheds there are the platform canopies infra.rs already draws, other
        // buildings become a roof on pillars over the tracks
        let station = on_rail.get(&key).copied().unwrap_or(0) * 10 >= all.len() as u32 * 3;
        // under a viaduct (shops under the tracks, under an expressway); a deck
        // that only touches the building (a footbridge into its 2nd floor) does not cut it
        let clip = !station && under_deck.get(&key).copied().unwrap_or(0) * 10 >= all.len() as u32 * 3;
        if station && r.src == 3 {
            continue;
        }
        // railways and roads on structures keep their columns (a station roof spans them)
        let ps: Vec<usize> = all.iter().copied().filter(|&i| station || cols[i].flags & (CF_RAIL | CF_ROAD) == 0).collect();
        if ps.is_empty() {
            continue;
        }
        let mut m = materials(&r);
        if station {
            // light platform roofing on slim steel-grey pillars
            m.roof = b("smooth_stone");
            m.frame = b("light_gray_concrete");
        }
        let base = SEA_Y + r.base.round() as i32;
        let height = r.height.max(2.5);
        let storeys = (r.storeys as i32).max(1);
        let top = base + height.round().max(2.0) as i32; // eave / flat roof level
        let sh = ((height / storeys as f32).floor() as i32).clamp(2, 6); // storey height
        let inb = |j: i64| -> bool { j >= 0 && (j as usize) < n * n && fx.bldg[j as usize] == key };
        let open = r.roof == features::R_OPEN || station;
        // roof height of every pixel: LOD2 surfaces (gaps filled from the
        // nearest roof pixel of the same building), gable or flat
        let local: HashMap<usize, usize> = ps.iter().enumerate().map(|(k, &i)| (i, k)).collect();
        let mut lod2: Vec<f32> = ps.iter().map(|&i| if r.roof == features::R_LOD2 { fx.roof[i] } else { f32::NAN }).collect();
        if r.roof == features::R_LOD2 && lod2.iter().any(|v| v.is_finite()) && lod2.iter().any(|v| !v.is_finite()) {
            let mut q: std::collections::VecDeque<usize> = (0..ps.len()).filter(|&k| lod2[k].is_finite()).collect();
            while let Some(k) = q.pop_front() {
                let i = ps[k] as i64;
                for d in [-1i64, 1, -nn, nn] {
                    if let Some(&kk) = local.get(&((i + d) as usize)) {
                        if !lod2[kk].is_finite() {
                            lod2[kk] = lod2[k];
                            q.push_back(kk);
                        }
                    }
                }
            }
        }
        let ridge_x = r.ridge_x & 1 == 1;
        let mut roof: Vec<i32> = Vec::with_capacity(ps.len());
        for (k, &i) in ps.iter().enumerate() {
            let (x, z) = ((i % n) as i64, (i / n) as i64);
            let mut ry = match r.roof {
                features::R_LOD2 if lod2[k].is_finite() => base + lod2[k].round().max(2.0) as i32,
                features::R_GABLE => {
                    // distance (in pixels) to the edge perpendicular to the ridge
                    let mut d = 0i64;
                    loop {
                        let (a, b2) = if ridge_x { ((z - d - 1) * nn + x, (z + d + 1) * nn + x) } else { (z * nn + x - d - 1, z * nn + x + d + 1) };
                        let ok_a = if ridge_x { z - d - 1 >= 0 } else { x - d - 1 >= 0 };
                        let ok_b = if ridge_x { z + d + 1 < nn } else { x + d + 1 < nn };
                        if !(ok_a && inb(a) && ok_b && inb(b2)) || d > 12 {
                            break;
                        }
                        d += 1;
                    }
                    top + 1 + (d as f32 * 0.6) as i32
                }
                _ => top + 1,
            };
            // under a viaduct: the roof just below the deck (a station roof stays above it)
            let db = deck_bottom[i];
            if db != i32::MAX && db > base && clip && ry >= db {
                ry = db - 1;
            }
            roof.push(ry);
        }
        let mut entrance: Option<usize> = None;
        for (k, &i) in ps.iter().enumerate() {
            let (x, z) = ((i % n) as i64, (i / n) as i64);
            let roof_y = roof[k];
            // what the roof stands on: the ground, or a deck / platform below it
            let floor = if station { ov.v[i].iter().map(|e| e.0).filter(|&y| y < roof_y).max().unwrap_or(i32::MIN).max(cols[i].top_y()) } else { base };
            if roof_y <= floor + 1 || (station && roof_y < floor + 4) {
                continue; // no room (under a deck, or a station roof below its platform)
            }
            if !station {
                // ground: floor slab at `base`, terrain above it removed
                let c = &mut cols[i];
                c.g = base;
                c.w = base;
                c.top = m.floor;
                c.sub = ids.stone;
                c.sub_n = 2;
                c.flags |= CF_BLDG | CF_NO_TREE | CF_NO_PLANT;
            } else {
                cols[i].flags |= CF_NO_TREE;
            }
            // the wall is exposed above the lowest neighbouring roof (the
            // ground outside the footprint), so set-backs, towers on a podium
            // and courtyards get facades instead of open floors
            let mut outer = false;
            let mut lo = roof_y;
            for (dx, dz) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                let (xx, zz) = (x + dx, z + dz);
                if xx < 0 || zz < 0 || xx >= nn || zz >= nn {
                    continue;
                }
                match local.get(&((zz * nn + xx) as usize)) {
                    Some(&kk) => lo = lo.min(roof[kk]),
                    None => {
                        // outside the footprint, or a part of it that is not drawn (tracks, water)
                        outer |= !inb(zz * nn + xx);
                        lo = floor;
                    }
                }
            }
            for y in floor + 1..roof_y {
                if y > lo {
                    let level = (y - base - 1).rem_euclid(sh);
                    let window_row = level == 1 || (sh >= 4 && level == 2);
                    let pillar = (x + z) % 4 == 0;
                    let blk = if open {
                        // pillars, never on the tracks
                        if pillar && !(station && matches!(fx.rail[i], 1..=3)) { m.frame } else { continue }
                    } else if y > top {
                        m.wall // gable end, rooftop structures
                    } else if window_row && (x + z) % 3 != 0 && r.usage != features::U_GREENHOUSE {
                        m.window
                    } else {
                        m.wall
                    };
                    ov.put(i, y, blk);
                } else if !open && (y - base) % sh == 0 && y < top && y < roof_y - 1 {
                    // floors between storeys
                    ov.put(i, y, m.floor);
                }
            }
            if outer && entrance.is_none() && !open {
                entrance = Some(i);
            }
            ov.put(i, roof_y, m.roof);
            if r.roof == features::R_FLAT && outer && height >= 6.0 && !open && deck_bottom[i] > roof_y + 1 {
                ov.put(i, roof_y + 1, m.frame); // parapet
            }
        }
        // a ground-floor opening so the building can be entered
        if let Some(i) = entrance {
            ov.put(i, base + 1, ids.air);
            ov.put(i, base + 2, ids.air);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn distance_is_exact_euclidean() {
        // pseudo-random sources on grids of a few shapes, against brute force
        for (w, h, density, cap) in [(61usize, 47usize, 37u64, 1e9f32), (130, 9, 251, 1e9), (17, 140, 911, 12.5), (64, 64, 4001, 1e9)] {
            let src: Vec<bool> = (0..w * h).map(|i| super::hash3(i as i64, w as i64, density) % density as u32 == 0).collect();
            let d = super::distance(&src, w, h, cap);
            for y in 0..h {
                for x in 0..w {
                    let mut best = f64::INFINITY;
                    for yy in 0..h {
                        for xx in 0..w {
                            if src[yy * w + xx] {
                                let dd = ((x as f64 - xx as f64).powi(2) + (y as f64 - yy as f64).powi(2)).sqrt();
                                best = best.min(dd);
                            }
                        }
                    }
                    let want = (best as f32).min(cap);
                    let got = d[y * w + x];
                    assert!((got - want).abs() < 1e-3 || (want >= cap && got >= cap), "({x},{y}) {w}x{h}: got {got}, want {want}");
                }
            }
        }
    }
}
