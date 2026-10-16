//! Parsing of GSI 基盤地図情報 GML (JPGIS 2.1) — DEM coverages and water-area polygons.

use anyhow::{Context, Result, anyhow};

/// Elevation cell codes (f32). Real elevations are > -1000.
pub const SEA: f32 = -30000.0;
pub const INLAND: f32 = -20000.0;
pub const NODATA: f32 = f32::NAN;

#[inline]
pub fn is_elev(v: f32) -> bool {
    v > -1000.0
}

fn find(h: &[u8], pat: &[u8], from: usize) -> Option<usize> {
    if from >= h.len() {
        return None;
    }
    memchr_find(&h[from..], pat).map(|p| p + from)
}

fn memchr_find(h: &[u8], pat: &[u8]) -> Option<usize> {
    let first = pat[0];
    let mut i = 0;
    while i + pat.len() <= h.len() {
        match h[i..].iter().position(|&b| b == first) {
            None => return None,
            Some(p) => {
                i += p;
                if i + pat.len() <= h.len() && &h[i..i + pat.len()] == pat {
                    return Some(i);
                }
                i += 1;
            }
        }
    }
    None
}

/// Text between `<tag ...>` and `</tag>` starting the search at `from`.
fn tag_text<'a>(h: &'a [u8], tag: &str, from: usize) -> Option<(&'a [u8], usize)> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let s = find(h, open.as_bytes(), from)?;
    let gt = find(h, b">", s)? + 1;
    let e = find(h, close.as_bytes(), gt)?;
    Some((&h[gt..e], e + close.len()))
}

fn txt(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim().to_string()
}

#[derive(Clone, Debug, Default)]
pub struct DemMeta {
    pub fmdid: String,
    pub srs: String,
    pub kind: String,
    pub mesh: String,
    pub date: String,
    pub lat_min: f64,
    pub lon_min: f64,
    pub lat_max: f64,
    pub lon_max: f64,
    pub nx: usize,
    pub ny: usize,
}

/// Parse just the header of a DEM GML (works on a truncated prefix).
pub fn dem_meta(h: &[u8]) -> Result<DemMeta> {
    let mut m = DemMeta::default();
    if let Some((t, _)) = tag_text(h, "gml:description", 0) {
        let s = txt(t);
        m.fmdid = s.rsplit('=').next().unwrap_or("").trim().to_string();
    }
    let p = find(h, b"srsName=\"", 0).ok_or_else(|| anyhow!("no srsName"))? + 9;
    let q = find(h, b"\"", p).unwrap();
    m.srs = txt(&h[p..q]);
    m.kind = tag_text(h, "type", 0).map(|(t, _)| txt(t)).unwrap_or_default();
    m.mesh = tag_text(h, "mesh", 0).map(|(t, _)| txt(t)).unwrap_or_default();
    m.date = tag_text(h, "gml:timePosition", 0).map(|(t, _)| txt(t)).unwrap_or_default();
    let lc = txt(tag_text(h, "gml:lowerCorner", 0).ok_or_else(|| anyhow!("no lowerCorner"))?.0);
    let uc = txt(tag_text(h, "gml:upperCorner", 0).ok_or_else(|| anyhow!("no upperCorner"))?.0);
    let hi = txt(tag_text(h, "gml:high", 0).ok_or_else(|| anyhow!("no high"))?.0);
    let f = |s: &str| -> Vec<f64> { s.split_whitespace().filter_map(|x| x.parse().ok()).collect() };
    let (l, u, g) = (f(&lc), f(&uc), f(&hi));
    m.lat_min = l[0];
    m.lon_min = l[1];
    m.lat_max = u[0];
    m.lon_max = u[1];
    m.nx = g[0] as usize + 1;
    m.ny = g[1] as usize + 1;
    Ok(m)
}

#[inline]
fn parse_num(b: &[u8]) -> f32 {
    // fast path for "[-]ddd[.dd]"
    let mut i = 0;
    let neg = !b.is_empty() && b[0] == b'-';
    if neg {
        i = 1;
    }
    let mut ip: i64 = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        ip = ip * 10 + (b[i] - b'0') as i64;
        i += 1;
    }
    let mut frac: i64 = 0;
    let mut scale: i64 = 1;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            frac = frac * 10 + (b[i] - b'0') as i64;
            scale *= 10;
            i += 1;
        }
    }
    let v = ip as f64 + frac as f64 / scale as f64;
    (if neg { -v } else { v }) as f32
}

const T_SEA: &[u8] = "海水面".as_bytes();
const T_INLAND: &[u8] = "内水面".as_bytes();
const T_NONE: &[u8] = "データなし".as_bytes();

/// One DEM coverage: header + values (row-major, north row first). Cells not
/// listed are NODATA; sea/inland-water points get SEA/INLAND codes.
pub struct DemGrid {
    pub meta: DemMeta,
    pub values: Vec<f32>,
}

pub fn parse_dem(h: &[u8]) -> Result<Vec<DemGrid>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(s) = find(h, b"<DEM ", pos) {
        let e = find(h, b"</DEM>", s).ok_or_else(|| anyhow!("unterminated DEM"))?;
        let body = &h[s..e];
        let meta = dem_meta(body).context("DEM header")?;
        let (tl, after) = tag_text(body, "gml:tupleList", 0).ok_or_else(|| anyhow!("no tupleList"))?;
        let sp = tag_text(body, "gml:startPoint", after).map(|(t, _)| txt(t)).unwrap_or_else(|| "0 0".into());
        let spv: Vec<usize> = sp.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        let (nx, ny) = (meta.nx, meta.ny);
        let mut values = vec![NODATA; nx * ny];
        let mut idx = spv.get(1).copied().unwrap_or(0) * nx + spv.first().copied().unwrap_or(0);
        for line in tl.split(|&b| b == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let Some(c) = line.iter().position(|&b| b == b',') else { continue };
            if idx >= values.len() {
                break;
            }
            let ty = &line[..c];
            let v = parse_num(&line[c + 1..]);
            values[idx] = if ty == T_SEA {
                SEA
            } else if ty == T_INLAND {
                INLAND
            } else if ty == T_NONE || v <= -9000.0 {
                NODATA
            } else {
                v
            };
            idx += 1;
        }
        out.push(DemGrid { meta, values });
        pos = e;
    }
    Ok(out)
}

// ---------------------------------------------------------------- 基本項目 features

/// A generic 基盤地図情報 基本項目 feature (lines or polygon rings in lat/lon).
#[derive(Clone, Debug, Default)]
pub struct KFeature {
    pub kind: String,      // element name, e.g. "RdEdg"
    pub ty: String,        // <type>
    pub extra: String,     // e.g. <admOffice> / <name>
    pub lines: Vec<Vec<(f64, f64)>>,
    pub rings: Vec<Vec<(f64, f64)>>, // first ring exterior (areas only)
}

/// Parse every `<tag ...>` element of a 基本項目 GML file. Curves become
/// `lines`; surfaces become `rings` (exterior first, then interiors).
pub fn parse_kihon(h: &[u8], tag: &str) -> Vec<KFeature> {
    let open = format!("<{tag} ");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(s) = find(h, open.as_bytes(), pos) {
        let Some(e) = find(h, close.as_bytes(), s) else { break };
        let body = &h[s..e];
        pos = e;
        let mut f = KFeature { kind: tag.to_string(), ..Default::default() };
        f.ty = tag_text(body, "type", 0).map(|(t, _)| txt(t)).unwrap_or_default();
        f.extra = tag_text(body, "admOffice", 0).or_else(|| tag_text(body, "name", 0)).map(|(t, _)| txt(t)).unwrap_or_default();
        if find(body, b"<gml:Surface", 0).is_some() {
            let mut p = 0;
            loop {
                let ex = find(body, b"<gml:exterior>", p);
                let inn = find(body, b"<gml:interior>", p);
                let (start, cl): (usize, &[u8]) = match (ex, inn) {
                    (Some(a), Some(b)) if a < b => (a, b"</gml:exterior>"),
                    (Some(_), Some(b)) => (b, b"</gml:interior>"),
                    (Some(a), None) => (a, b"</gml:exterior>"),
                    (None, Some(b)) => (b, b"</gml:interior>"),
                    (None, None) => break,
                };
                let Some(end) = find(body, cl, start) else { break };
                let mut ring = Vec::new();
                let mut q = start;
                while let Some((t, nq)) = tag_text(&body[..end], "gml:posList", q) {
                    parse_poslist(t, &mut ring);
                    q = nq;
                }
                if ring.len() >= 3 {
                    f.rings.push(ring);
                }
                p = end;
            }
        } else {
            let mut q = 0;
            while let Some((t, nq)) = tag_text(body, "gml:posList", q) {
                let mut line = Vec::new();
                parse_poslist(t, &mut line);
                if line.len() >= 2 {
                    f.lines.push(line);
                }
                q = nq;
            }
        }
        if !f.lines.is_empty() || !f.rings.is_empty() {
            out.push(f);
        }
    }
    out
}

// ---------------------------------------------------------------- water areas

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum WaterKind {
    Sea = 1,
    Inland = 2,
}

pub struct WaterPoly {
    pub kind: WaterKind,
    /// rings in (lat, lon); first is exterior
    pub rings: Vec<Vec<(f64, f64)>>,
}

fn parse_poslist(t: &[u8], out: &mut Vec<(f64, f64)>) {
    let s = std::str::from_utf8(t).unwrap_or("");
    let mut it = s.split_ascii_whitespace();
    while let (Some(a), Some(b)) = (it.next(), it.next()) {
        if let (Ok(la), Ok(lo)) = (a.parse::<f64>(), b.parse::<f64>()) {
            out.push((la, lo));
        }
    }
}

/// Parse `WA` (水域) features. Returns polygons plus counts of each type string.
pub fn parse_wa(h: &[u8], types: &mut std::collections::BTreeMap<String, usize>) -> Vec<WaterPoly> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(s) = find(h, b"<WA ", pos) {
        let Some(e) = find(h, b"</WA>", s) else { break };
        let body = &h[s..e];
        pos = e;
        let ty = tag_text(body, "type", 0).map(|(t, _)| txt(t)).unwrap_or_default();
        *types.entry(ty.clone()).or_default() += 1;
        let kind = if ty == "海" {
            WaterKind::Sea
        } else if ty.contains("河川") || ty.contains("湖") || ty.contains("池") {
            WaterKind::Inland
        } else {
            continue;
        };
        let mut rings = Vec::new();
        let mut p = 0;
        loop {
            let ex = find(body, b"<gml:exterior>", p);
            let inn = find(body, b"<gml:interior>", p);
            let (start, close): (usize, &[u8]) = match (ex, inn) {
                (Some(a), Some(b)) if a < b => (a, b"</gml:exterior>"),
                (Some(_), Some(b)) => (b, b"</gml:interior>"),
                (Some(a), None) => (a, b"</gml:exterior>"),
                (None, Some(b)) => (b, b"</gml:interior>"),
                (None, None) => break,
            };
            let Some(end) = find(body, close, start) else { break };
            let ring_txt = &body[start..end];
            let mut ring = Vec::new();
            let mut q = 0;
            while let Some((t, nq)) = tag_text(ring_txt, "gml:posList", q) {
                parse_poslist(t, &mut ring);
                q = nq;
            }
            if ring.len() >= 3 {
                rings.push(ring);
            }
            p = end;
        }
        if !rings.is_empty() {
            out.push(WaterPoly { kind, rings });
        }
    }
    out
}
