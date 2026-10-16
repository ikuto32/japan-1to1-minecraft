//! Perspective renders of a generated world, for checking bridges, tunnels and
//! buildings without the game client: rays are marched through the voxels of
//! the region files (water and glass tint, slabs and rails are thin), faces are
//! shaded like the game does (top bright, sides darker) and fade into the sky.
//!
//! `jpdem verify view <region dir> X Y Z yaw pitch out.png [W H fov maxdist]`
//! uses the game's camera convention (yaw 0 = facing south, 90 = west;
//! pitch > 0 looks down; vertical field of view in degrees).

use crate::nbt::{self, Tag};
use anyhow::Result;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::Path;

const MIN_Y: i32 = -2032;
const N_SEC: usize = 254;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Air,
    Solid,
    Water,
    Glass,
    /// thin block whose top is this far above its bottom (slabs, rails, carpets, plants)
    Partial(f32),
}

#[derive(Clone, Copy)]
struct Info {
    rgb: [f32; 3],
    kind: Kind,
}

fn info_of(name: &str, props: &[(String, String)]) -> Info {
    let n = name.trim_start_matches("minecraft:");
    let rgb = crate::verify::block_color(name).map(|v| v as f32 / 255.0);
    let prop = |k: &str| props.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str());
    let kind = if n == "air" || n == "cave_air" || n == "void_air" || n.is_empty() {
        Kind::Air
    } else if n == "water" {
        Kind::Water
    } else if n.ends_with("glass") || n.ends_with("glass_pane") {
        Kind::Glass
    } else if n.ends_with("_slab") {
        match prop("type") {
            Some("bottom") => Kind::Partial(0.5),
            _ => Kind::Solid,
        }
    } else if n.ends_with("rail") {
        Kind::Partial(0.12)
    } else if n.ends_with("_carpet") || n == "daylight_detector" {
        Kind::Partial(0.38)
    } else if matches!(n, "short_grass" | "fern" | "tall_grass" | "large_fern" | "poppy" | "dandelion" | "azure_bluet" | "oxeye_daisy" | "cornflower" | "seagrass" | "wheat" | "carrots" | "potatoes" | "beetroots") {
        Kind::Partial(0.3)
    } else if n.ends_with("_wall") || n.ends_with("_fence") || n == "bamboo" {
        Kind::Solid
    } else {
        Kind::Solid
    };
    Info { rgb, kind }
}

/// Block palette indices of the loaded chunks (index 0 = air everywhere).
struct Vox {
    infos: Vec<Info>,
    /// chunk (cx, cz) -> 254 optional sections of 4096 palette indices
    chunks: HashMap<(i64, i64), Vec<Option<Box<[u16]>>>>,
}

impl Vox {
    #[inline]
    fn at(&self, x: i64, y: i64, z: i64) -> Info {
        let sy = (y - MIN_Y as i64).div_euclid(16);
        if sy < 0 || sy >= N_SEC as i64 {
            return self.infos[0];
        }
        let Some(c) = self.chunks.get(&(x.div_euclid(16), z.div_euclid(16))) else { return self.infos[0] };
        let Some(s) = &c[sy as usize] else { return self.infos[0] };
        let i = (y.rem_euclid(16) * 256 + z.rem_euclid(16) * 16 + x.rem_euclid(16)) as usize;
        self.infos[s[i] as usize]
    }
}

/// Decode every chunk of one region file whose chunk coords pass `want`.
fn load_region(path: &Path, want: &(dyn Fn(i64, i64) -> bool + Sync), rx: i64, rz: i64) -> Result<Vec<((i64, i64), Vec<(usize, Vec<u16>)>, Vec<(String, Vec<(String, String)>)>)>> {
    let Ok(d) = std::fs::read(path) else { return Ok(vec![]) };
    let mut out = Vec::new();
    for i in 0..1024usize {
        let (lx, lz) = (i % 32, i / 32);
        let (cx, cz) = (rx * 32 + lx as i64, rz * 32 + lz as i64);
        if !want(cx, cz) {
            continue;
        }
        let off = ((d[i * 4] as usize) << 16 | (d[i * 4 + 1] as usize) << 8 | d[i * 4 + 2] as usize) * 4096;
        if off == 0 || off + 5 > d.len() {
            continue;
        }
        let len = u32::from_be_bytes(d[off..off + 4].try_into()?) as usize;
        let mut raw = Vec::new();
        use std::io::Read;
        flate2::read::ZlibDecoder::new(&d[off + 5..off + 4 + len]).read_to_end(&mut raw)?;
        let (_, t) = nbt::read_root(&raw)?;
        let Some(Tag::List(secs)) = t.get("sections") else { continue };
        let mut pal: Vec<(String, Vec<(String, String)>)> = Vec::new();
        let mut sections = Vec::new();
        for s in secs {
            let Some(Tag::Byte(y)) = s.get("Y") else { continue };
            let Some(bs) = s.get("block_states") else { continue };
            let Some(Tag::List(p)) = bs.get("palette") else { continue };
            let base = pal.len() as u16;
            for e in p {
                let name = match e.get("Name") {
                    Some(Tag::String(s)) => s.clone(),
                    _ => String::new(),
                };
                let props = match e.get("Properties") {
                    Some(Tag::Compound(v)) => v.iter().filter_map(|(k, v)| if let Tag::String(s) = v { Some((k.clone(), s.clone())) } else { None }).collect(),
                    _ => vec![],
                };
                pal.push((name, props));
            }
            if p.len() == 1 && pal[base as usize].0 == "minecraft:air" {
                continue;
            }
            let mut v = vec![base; 4096];
            if let Some(Tag::LongArray(data)) = bs.get("data") {
                let bits = ((usize::BITS - (p.len() - 1).leading_zeros()) as usize).max(4);
                let per = 64 / bits;
                for (k, x) in v.iter_mut().enumerate() {
                    *x = base + (((data[k / per] as u64) >> ((k % per) * bits)) & ((1u64 << bits) - 1)) as u16;
                }
            }
            let si = *y as i32 - MIN_Y / 16;
            if si >= 0 && (si as usize) < N_SEC {
                sections.push((si as usize, v));
            }
        }
        out.push(((cx, cz), sections, pal));
    }
    Ok(out)
}

pub struct Cam {
    pub pos: [f64; 3],
    pub yaw: f64,
    pub pitch: f64,
    pub fov: f64,
    pub w: usize,
    pub h: usize,
    pub maxd: f64,
}

pub fn render(dir: &Path, cam: &Cam, out: &Path) -> Result<()> {
    let r = cam.maxd;
    let (cx0, cx1) = (((cam.pos[0] - r) / 16.0).floor() as i64, ((cam.pos[0] + r) / 16.0).floor() as i64);
    let (cz0, cz1) = (((cam.pos[2] - r) / 16.0).floor() as i64, ((cam.pos[2] + r) / 16.0).floor() as i64);
    let want = move |cx: i64, cz: i64| cx >= cx0 && cx <= cx1 && cz >= cz0 && cz <= cz1;
    let regs: Vec<(i64, i64)> = (cz0.div_euclid(32)..=cz1.div_euclid(32)).flat_map(|rz| (cx0.div_euclid(32)..=cx1.div_euclid(32)).map(move |rx| (rx, rz))).collect();
    let loaded: Vec<_> = regs
        .par_iter()
        .map(|&(rx, rz)| load_region(&dir.join(format!("r.{rx}.{rz}.mca")), &want, rx, rz))
        .collect::<Result<Vec<_>>>()?;
    let mut vox = Vox { infos: vec![Info { rgb: [0.0; 3], kind: Kind::Air }], chunks: HashMap::new() };
    let mut gidx: HashMap<String, u16> = HashMap::new();
    for reg in loaded {
        for (key, secs, pal) in reg {
            let map: Vec<u16> = pal
                .iter()
                .map(|(n, p)| {
                    let k = format!("{n}{p:?}");
                    *gidx.entry(k).or_insert_with(|| {
                        vox.infos.push(info_of(n, p));
                        (vox.infos.len() - 1) as u16
                    })
                })
                .collect();
            let mut col: Vec<Option<Box<[u16]>>> = vec![None; N_SEC];
            for (si, v) in secs {
                col[si] = Some(v.into_iter().map(|i| map[i as usize]).collect::<Vec<u16>>().into_boxed_slice());
            }
            vox.chunks.insert(key, col);
        }
    }
    eprintln!("view: {} chunks, {} block states", vox.chunks.len(), vox.infos.len());
    let (yaw, pitch) = (cam.yaw.to_radians(), cam.pitch.to_radians());
    let f = [-yaw.sin() * pitch.cos(), -pitch.sin(), yaw.cos() * pitch.cos()];
    let up = [0.0, 1.0, 0.0];
    let cross = |a: [f64; 3], b: [f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let norm = |a: [f64; 3]| {
        let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
        [a[0] / l, a[1] / l, a[2] / l]
    };
    let rgt = norm(cross(f, up));
    let upv = cross(rgt, f);
    let th = (cam.fov.to_radians() / 2.0).tan();
    let aspect = cam.w as f64 / cam.h as f64;
    let sky = [0.62f32, 0.75, 0.95];
    let mut rgb = vec![0u8; cam.w * cam.h * 3];
    rgb.par_chunks_mut(cam.w * 3).enumerate().for_each(|(py, row)| {
        for px in 0..cam.w {
            let u = (2.0 * (px as f64 + 0.5) / cam.w as f64 - 1.0) * th * aspect;
            let v = (1.0 - 2.0 * (py as f64 + 0.5) / cam.h as f64) * th;
            let d = norm([f[0] + u * rgt[0] + v * upv[0], f[1] + u * rgt[1] + v * upv[1], f[2] + u * rgt[2] + v * upv[2]]);
            let c = trace(&vox, cam.pos, d, cam.maxd, sky);
            for k in 0..3 {
                row[px * 3 + k] = (c[k].clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
    });
    crate::raster::write_png(out, cam.w, cam.h, &rgb)
}

fn trace(vox: &Vox, o: [f64; 3], d: [f64; 3], maxd: f64, sky: [f32; 3]) -> [f32; 3] {
    let mut p = [o[0].floor() as i64, o[1].floor() as i64, o[2].floor() as i64];
    let step = [d[0].signum() as i64, d[1].signum() as i64, d[2].signum() as i64];
    let tdelta = [1.0 / d[0].abs().max(1e-12), 1.0 / d[1].abs().max(1e-12), 1.0 / d[2].abs().max(1e-12)];
    let mut tmax = [0.0f64; 3];
    for a in 0..3 {
        tmax[a] = if d[a] > 0.0 { (p[a] as f64 + 1.0 - o[a]) * tdelta[a] } else if d[a] < 0.0 { (o[a] - p[a] as f64) * tdelta[a] } else { f64::INFINITY };
    }
    let mut t_in = 0.0f64;
    let mut axis = 1usize; // axis of the face the ray entered the current voxel through
    let (mut acc, mut trans) = ([0f32; 3], 1f32);
    let shade = |axis: usize| -> f32 {
        match axis {
            1 => if d[1] < 0.0 { 1.0 } else { 0.5 },
            0 => 0.62,
            _ => 0.8,
        }
    };
    let fog = |t: f64| ((t / maxd) as f32).powf(1.6).clamp(0.0, 1.0);
    while t_in < maxd {
        let b = vox.at(p[0], p[1], p[2]);
        let t_out = tmax[0].min(tmax[1]).min(tmax[2]);
        let hit = |rgb: [f32; 3], t: f64, s: f32, acc: [f32; 3], trans: f32| -> [f32; 3] {
            let fg = fog(t);
            let mut c = [0f32; 3];
            for k in 0..3 {
                let surf = rgb[k] * s * (1.0 - fg) + sky[k] * fg;
                c[k] = acc[k] + trans * surf;
            }
            c
        };
        match b.kind {
            Kind::Air => {}
            Kind::Solid => return hit(b.rgb, t_in, shade(axis), acc, trans),
            Kind::Partial(h) => {
                let y_in = o[1] + d[1] * t_in;
                let top = p[1] as f64 + h as f64;
                if y_in <= top + 1e-9 {
                    return hit(b.rgb, t_in, shade(axis), acc, trans);
                }
                if d[1] < 0.0 {
                    let tp = (top - o[1]) / d[1];
                    if tp <= t_out {
                        return hit(b.rgb, tp, 1.0, acc, trans);
                    }
                }
            }
            Kind::Water | Kind::Glass => {
                // like the game: the surface is a translucent tint, the water
                // below it hardly absorbs (the sea floor shows from above)
                let (a, col) = if b.kind == Kind::Water { (0.008f32, [0.16f32, 0.30, 0.62]) } else { (0.35, [0.75, 0.85, 0.9]) };
                let len = (t_out - t_in) as f32;
                let mut keep = (1.0 - a).powf(len.max(0.05));
                let entering = b.kind == Kind::Water && vox.at(p[0] - step[0] * (axis == 0) as i64, p[1] - step[1] * (axis == 1) as i64, p[2] - step[2] * (axis == 2) as i64).kind == Kind::Air;
                if entering {
                    keep *= 0.5;
                }
                for k in 0..3 {
                    acc[k] += trans * (1.0 - keep) * col[k] * shade(1);
                }
                trans *= keep;
                if trans < 0.02 {
                    return acc;
                }
            }
        }
        // next voxel
        let a = if tmax[0] < tmax[1] { if tmax[0] < tmax[2] { 0 } else { 2 } } else if tmax[1] < tmax[2] { 1 } else { 2 };
        t_in = tmax[a];
        tmax[a] += tdelta[a];
        p[a] += step[a];
        axis = a;
        if p[1] < MIN_Y as i64 - 1 || p[1] > 2100 {
            break;
        }
    }
    let mut c = acc;
    for k in 0..3 {
        c[k] += trans * sky[k];
    }
    c
}
