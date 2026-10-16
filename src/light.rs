//! Sky light of a region window as Minecraft's light engine computes it, so
//! every chunk is written lit (`isLightOn`) and the game does not have to
//! light it again when it is first loaded.
//!
//! The model (vanilla 1.20+ light engine): in every column the sky light is
//! 15 from the top down to the first edge that stops it (the block below
//! takes light away, or one of the two faces is closed by the block's shape,
//! e.g. the underside of a bottom slab). From there light spreads to the six
//! neighbours, losing max(1, opacity of the block it enters) per step, unless
//! the two touching faces together close the face. The result is the unique
//! fixed point, which a bucket queue reaches directly. There are no light
//! emitting blocks in the world, so there is no block light.
//!
//! Per column only a band is stored: above it everything is sky (15); below
//! it everything is solid or out of reach of any sky light (0).

use crate::blocks::{BlockId, LightProp};
use crate::rgen::{Col, Ids, Overlay};
use crate::world::MIN_Y;

pub struct SkyLight {
    /// band per column: [lo, hi); y >= hi is sky, y < lo is dark
    lo: Vec<i32>,
    hi: Vec<i32>,
    off: Vec<usize>,
    v: Vec<u8>,
    /// lowest y with light in each column (hi when the band is all dark)
    lit_lo: Vec<i32>,
}

impl SkyLight {
    #[inline]
    pub fn get(&self, i: usize, y: i32) -> u8 {
        if y >= self.hi[i] {
            15
        } else if y < self.lo[i] {
            0
        } else {
            self.v[self.off[i] + (y - self.lo[i]) as usize]
        }
    }
    /// first y of open sky above everything in the column
    #[inline]
    pub fn sky_from(&self, i: usize) -> i32 {
        self.hi[i]
    }
    /// lowest y that has any sky light in the column
    #[inline]
    pub fn lit_from(&self, i: usize) -> i32 {
        self.lit_lo[i]
    }
}

/// Faces between a voxel and its neighbour in direction `d` (0 down, 1 up,
/// 2.. sides) closed for light by the two blocks' shapes.
#[inline]
fn occluded(from: &LightProp, to: &LightProp, d: usize) -> bool {
    match d {
        0 => from.down || to.up,
        1 => from.up || to.down,
        _ => {
            let (a0, a1) = from.side;
            let (b0, b1) = to.side;
            let full = |lo: u8, hi: u8| lo == 0 && hi >= 16;
            full(a0, a1) || full(b0, b1) || (a1 > a0 && b1 > b0 && ((a0 == 0 && b1 >= 16 && a1 >= b0) || (b0 == 0 && a1 >= 16 && b1 >= a0)))
        }
    }
}

/// Separable minimum over a (2r+1)² window, clamped at the edges
/// (monotonic deque per line).
fn min_filter(v: &[i32], n: usize, r: usize) -> Vec<i32> {
    fn line(src: &[i32], dst: &mut [i32], r: usize, q: &mut std::collections::VecDeque<usize>) {
        let len = src.len();
        q.clear();
        let mut next = 0;
        for i in 0..len {
            while next < len && next <= i + r {
                while q.back().is_some_and(|&b| src[b] >= src[next]) {
                    q.pop_back();
                }
                q.push_back(next);
                next += 1;
            }
            while q.front().is_some_and(|&f| f + r < i) {
                q.pop_front();
            }
            dst[i] = src[*q.front().unwrap()];
        }
    }
    let mut q = std::collections::VecDeque::new();
    let mut a = vec![0i32; n * n];
    for z in 0..n {
        line(&v[z * n..(z + 1) * n], &mut a[z * n..(z + 1) * n], r, &mut q);
    }
    let mut b = vec![0i32; n * n];
    let (mut col, mut out) = (vec![0i32; n], vec![0i32; n]);
    for x in 0..n {
        for z in 0..n {
            col[z] = a[z * n + x];
        }
        line(&col, &mut out, r, &mut q);
        for z in 0..n {
            b[z * n + x] = out[z];
        }
    }
    b
}

/// Sky light of the `n`×`n` window. `props` holds the light properties of
/// every block id; `ov` must be finished (voxels sorted by y).
pub fn compute(n: usize, cols: &[Col], ov: &Overlay, props: &[LightProp], ids: &Ids) -> SkyLight {
    let air = props[ids.air as usize];
    let base_top = |c: &Col| if c.out { MIN_Y - 1 } else { c.top_y() };
    // top of everything in each column, and where its sky source column ends
    let mut hi = vec![MIN_Y; n * n];
    let mut src = vec![MIN_Y; n * n];
    for i in 0..n * n {
        let c = &cols[i];
        let vox = &ov.v[i];
        let bt = base_top(c);
        let top_ov = vox.iter().rev().find(|e| e.1 != ids.air).map(|e| e.0).unwrap_or(MIN_Y - 1);
        hi[i] = bt.max(top_ov) + 1;
        // walk down from the sky to the first edge that stops it
        let mut y = hi[i] - 1;
        let mut above = air;
        let mut k = vox.len();
        let mut s = MIN_Y;
        while y >= MIN_Y {
            while k > 0 && vox[k - 1].0 > y {
                k -= 1;
            }
            let here = k > 0 && vox[k - 1].0 == y;
            if !here && y > bt {
                // open air down to the next voxel or the ground (unless the
                // block above closes its underside, like a bottom slab)
                if above.down {
                    s = y + 1;
                    break;
                }
                above = air;
                y = if k > 0 { vox[k - 1].0 } else { MIN_Y - 1 }.max(bt);
                continue;
            }
            let b = if here { vox[k - 1].1 } else { c.block(y, ids) };
            let p = &props[b as usize];
            if p.opacity != 0 || above.down || p.up {
                s = y + 1;
                break;
            }
            above = *p;
            y -= 1;
        }
        // the sky source starts at the band top at the latest
        src[i] = s.min(hi[i]);
    }
    // no sky light reaches 15 below the lowest source within 15 columns
    let reach = min_filter(&src, n, 15);
    // bands: from the lowest block that can hold light to the top
    let mut lo = vec![0i32; n * n];
    let mut off = vec![0usize; n * n];
    let mut len = 0usize;
    for i in 0..n * n {
        let c = &cols[i];
        let floor = if c.out {
            MIN_Y
        } else {
            let mut f = if props[c.top as usize].opacity >= 15 { c.g + 1 } else { c.g };
            if let Some(e) = ov.v[i].iter().find(|e| props[e.1 as usize].opacity < 15) {
                f = f.min(e.0);
            }
            f
        };
        lo[i] = floor.max(reach[i] - 15).min(src[i]);
        off[i] = len;
        len += (hi[i] - lo[i]) as usize;
    }
    // block ids in the bands: the column (air, water, ground), then its voxels
    let mut blk: Vec<BlockId> = vec![ids.air; len];
    for i in 0..n * n {
        let c = &cols[i];
        let (l0, h0, base) = (lo[i], hi[i], off[i]);
        if !c.out {
            let wtop = c.w.max(c.g);
            for y in l0..h0.min(wtop + 1) {
                blk[base + (y - l0) as usize] = if y > c.g { ids.water } else { c.block(y, ids) };
            }
        }
        let vox = &ov.v[i];
        let k = vox.partition_point(|e| e.0 < l0);
        for e in vox[k..].iter().take_while(|e| e.0 < h0) {
            blk[base + (e.0 - l0) as usize] = e.1;
        }
    }
    // the sky sources, and the light straight down below them
    let mut v = vec![0u8; len];
    let mut lit_lo = vec![0i32; n * n];
    for i in 0..n * n {
        let base = off[i];
        let (l0, h0, s) = (lo[i], hi[i], src[i]);
        for y in s..h0 {
            v[base + (y - l0) as usize] = 15;
        }
        let mut above = if s < h0 { props[blk[base + (s - l0) as usize] as usize] } else { air };
        let mut l = 15i32;
        let mut y = s - 1;
        while y >= l0 {
            let q = base + (y - l0) as usize;
            let p = props[blk[q] as usize];
            if occluded(&above, &p, 0) {
                break;
            }
            l -= p.opacity.max(1) as i32;
            if l <= 0 {
                break;
            }
            v[q] = l as u8;
            above = p;
            y -= 1;
        }
        lit_lo[i] = y + 1;
    }
    // columns with the same lit blocks and light can not light each other
    let sig: Vec<u64> = (0..n * n)
        .map(|i| {
            let mut h = 0x9E37_79B9_7F4A_7C15u64 ^ (src[i] as u64) ^ ((hi[i] as u64) << 21) ^ ((lit_lo[i] as u64) << 42);
            for y in lit_lo[i]..hi[i] {
                h = (h ^ blk[off[i] + (y - lo[i]) as usize] as u64).wrapping_mul(0x0100_0000_01B3);
            }
            h
        })
        .collect();
    let mut buckets: Vec<Vec<(u32, i32)>> = vec![Vec::new(); 16];
    // light `l` from a voxel with block `from` into voxel (j, y), direction d
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn spread(
        v: &mut [u8],
        lit_lo: &mut [i32],
        buckets: &mut [Vec<(u32, i32)>],
        (lo, src, off, blk, props): (&[i32], &[i32], &[usize], &[BlockId], &[LightProp]),
        l: u8,
        from: &LightProp,
        j: usize,
        y: i32,
        d: usize,
    ) {
        // only dark voxels of a band (below the column's sky source)
        if y < lo[j] || y >= src[j] {
            return;
        }
        let r = off[j] + (y - lo[j]) as usize;
        let to = &props[blk[r] as usize];
        let nl = l as i32 - to.opacity.max(1) as i32;
        if nl <= v[r] as i32 || occluded(from, to, d) {
            return;
        }
        v[r] = nl as u8;
        if y < lit_lo[j] {
            lit_lo[j] = y;
        }
        if nl >= 2 {
            buckets[nl as usize].push((j as u32, y));
        }
    }
    let ctx = (&lo[..], &src[..], &off[..], &blk[..], props);
    let light_at = |v: &[u8], i: usize, y: i32| -> u8 { if y >= hi[i] { 15 } else if y < lo[i] { 0 } else { v[off[i] + (y - lo[i]) as usize] } };
    let block_at = |i: usize, y: i32| -> LightProp { if y >= hi[i] { air } else { props[blk[off[i] + (y - lo[i]) as usize] as usize] } };
    // seeds: where a column is lit and its neighbour is darker
    for i in 0..n * n {
        let (x, z) = (i % n, i / n);
        for (d, j) in [(2usize, x > 0), (3, x + 1 < n), (4, z > 0), (5, z + 1 < n)]
            .into_iter()
            .filter(|e| e.1)
            .map(|(d, _)| (d, match d { 2 => i - 1, 3 => i + 1, 4 => i - n, _ => i + n }))
        {
            if sig[i] == sig[j] {
                continue;
            }
            for y in lit_lo[i].max(lo[j])..src[j] {
                let l = light_at(&v, i, y);
                if l >= 2 {
                    let from = block_at(i, y);
                    spread(&mut v, &mut lit_lo, &mut buckets, ctx, l, &from, j, y, d);
                }
            }
        }
    }
    for l in (2..=14u8).rev() {
        let b = std::mem::take(&mut buckets[l as usize]);
        for (i, y) in b {
            let i = i as usize;
            let q = off[i] + (y - lo[i]) as usize;
            if v[q] != l {
                continue;
            }
            let from = props[blk[q] as usize];
            spread(&mut v, &mut lit_lo, &mut buckets, ctx, l, &from, i, y - 1, 0);
            spread(&mut v, &mut lit_lo, &mut buckets, ctx, l, &from, i, y + 1, 1);
            let (x, z) = (i % n, i / n);
            if x > 0 {
                spread(&mut v, &mut lit_lo, &mut buckets, ctx, l, &from, i - 1, y, 2);
            }
            if x + 1 < n {
                spread(&mut v, &mut lit_lo, &mut buckets, ctx, l, &from, i + 1, y, 3);
            }
            if z > 0 {
                spread(&mut v, &mut lit_lo, &mut buckets, ctx, l, &from, i - n, y, 4);
            }
            if z + 1 < n {
                spread(&mut v, &mut lit_lo, &mut buckets, ctx, l, &from, i + n, y, 5);
            }
        }
    }
    SkyLight { lo, hi, off, v, lit_lo }
}
