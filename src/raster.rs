//! Raster helpers: scanline polygon fill, pull-push hole filling, PNG output.

use anyhow::Result;
use std::path::Path;

/// Fill a polygon (even-odd over all rings) given in pixel coordinates
/// (pixel (c, r) has its centre at (c + 0.5, r + 0.5)). `span(row, c0, c1)`
/// is called for each run of covered pixels [c0, c1) clipped to w × h.
pub fn fill_polygon(rings: &[Vec<(f64, f64)>], w: usize, h: usize, mut span: impl FnMut(usize, usize, usize)) {
    struct Edge {
        r0: i64,
        r1: i64, // inclusive row range
        x0: f64,
        y0: f64,
        dxdy: f64,
    }
    let mut edges: Vec<Edge> = Vec::new();
    for ring in rings {
        let n = ring.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let (xa, ya) = ring[i];
            let (xb, yb) = ring[(i + 1) % n];
            if ya == yb {
                continue;
            }
            let (x0, y0, x1, y1) = if ya < yb { (xa, ya, xb, yb) } else { (xb, yb, xa, ya) };
            // rows whose centre y = r + 0.5 satisfies y0 <= y < y1
            let r0 = (y0 - 0.5).ceil() as i64;
            let r1 = (y1 - 0.5).ceil() as i64 - 1;
            let r0c = r0.max(0);
            let r1c = r1.min(h as i64 - 1);
            if r0c > r1c {
                continue;
            }
            edges.push(Edge { r0: r0c, r1: r1c, x0, y0, dxdy: (x1 - x0) / (y1 - y0) });
        }
    }
    if edges.is_empty() {
        return;
    }
    edges.sort_by_key(|e| e.r0);
    let rmin = edges[0].r0;
    let rmax = edges.iter().map(|e| e.r1).max().unwrap();
    let mut active: Vec<usize> = Vec::new();
    let mut next = 0;
    let mut xs: Vec<f64> = Vec::new();
    for r in rmin..=rmax {
        while next < edges.len() && edges[next].r0 <= r {
            active.push(next);
            next += 1;
        }
        active.retain(|&i| edges[i].r1 >= r);
        if active.is_empty() {
            continue;
        }
        let y = r as f64 + 0.5;
        xs.clear();
        for &i in &active {
            let e = &edges[i];
            xs.push(e.x0 + (y - e.y0) * e.dxdy);
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for p in xs.chunks_exact(2) {
            let c0 = ((p[0] - 0.5).ceil().max(0.0)) as i64;
            let c1 = ((p[1] - 0.5).ceil().min(w as f64)) as i64;
            if c1 > c0 {
                span(r as usize, c0 as usize, c1 as usize);
            }
        }
    }
}

/// Pull-push scattered-data interpolation. `v` holds known values (NaN =
/// unknown); on return every cell holds a value (known cells unchanged).
/// If nothing is known the array is left as is.
pub fn pull_push(v: &mut [f32], w: usize, h: usize) {
    use rayon::prelude::*;
    // level 0 weights: 1 known, 0 unknown
    let mut levels: Vec<(Vec<f32>, Vec<f32>, usize, usize)> = Vec::new(); // (avg, wt, w, h)
    let wt0: Vec<f32> = v.par_iter().map(|x| if x.is_nan() { 0.0 } else { 1.0 }).collect();
    if !wt0.iter().any(|&x| x > 0.0) {
        return;
    }
    let avg0: Vec<f32> = v.par_iter().map(|x| if x.is_nan() { 0.0 } else { *x }).collect();
    levels.push((avg0, wt0, w, h));
    // pull
    loop {
        let (a, wt, lw, lh) = levels.last().unwrap();
        if *lw == 1 && *lh == 1 {
            break;
        }
        let (nw, nh) = (lw.div_ceil(2), lh.div_ceil(2));
        let mut na = vec![0f32; nw * nh];
        let mut nwt = vec![0f32; nw * nh];
        na.par_chunks_mut(nw).zip(nwt.par_chunks_mut(nw)).enumerate().for_each(|(y, (ra, rw))| {
            for x in 0..nw {
                let mut s = 0f32;
                let mut ws = 0f32;
                for dy in 0..2 {
                    let yy = 2 * y + dy;
                    if yy >= *lh {
                        continue;
                    }
                    for dx in 0..2 {
                        let xx = 2 * x + dx;
                        if xx >= *lw {
                            continue;
                        }
                        let k = yy * lw + xx;
                        s += a[k] * wt[k];
                        ws += wt[k];
                    }
                }
                if ws > 0.0 {
                    ra[x] = s / ws;
                    rw[x] = ws.min(1.0);
                }
            }
        });
        levels.push((na, nwt, nw, nh));
    }
    // push: from coarse to fine, blend with bilinear-upsampled coarser level
    for l in (0..levels.len() - 1).rev() {
        let (upper, rest) = levels.split_at_mut(l + 1);
        let (ca, _, cw, ch) = &rest[0];
        let (fa, fwt, fw, fh) = &mut upper[l];
        let (cw, ch, fw2, _fh) = (*cw, *ch, *fw, *fh);
        fa.par_chunks_mut(fw2).zip(fwt.par_chunks_mut(fw2)).enumerate().for_each(|(y, (ra, rw))| {
            let cy = (y as f32 + 0.5) / 2.0 - 0.5;
            let y0 = cy.floor().max(0.0) as usize;
            let y1 = (y0 + 1).min(ch - 1);
            let fy = (cy - y0 as f32).clamp(0.0, 1.0);
            for x in 0..fw2 {
                let wv = rw[x];
                if wv >= 1.0 {
                    continue;
                }
                let cx = (x as f32 + 0.5) / 2.0 - 0.5;
                let x0 = cx.floor().max(0.0) as usize;
                let x1 = (x0 + 1).min(cw - 1);
                let fx = (cx - x0 as f32).clamp(0.0, 1.0);
                let t = ca[y0 * cw + x0] * (1.0 - fx) + ca[y0 * cw + x1] * fx;
                let b = ca[y1 * cw + x0] * (1.0 - fx) + ca[y1 * cw + x1] * fx;
                let up = t * (1.0 - fy) + b * fy;
                ra[x] = wv * ra[x] + (1.0 - wv) * up;
                rw[x] = 1.0;
            }
        });
    }
    let a0 = &levels[0].0;
    v.par_iter_mut().zip(a0.par_iter()).for_each(|(o, a)| {
        if o.is_nan() {
            *o = *a;
        }
    });
}

/// Write an RGB8 PNG.
pub fn write_png(path: &Path, w: usize, h: usize, rgb: &[u8]) -> Result<()> {
    let f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut e = png::Encoder::new(f, w as u32, h as u32);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    let mut wr = e.write_header()?;
    wr.write_image_data(rgb)?;
    Ok(())
}
