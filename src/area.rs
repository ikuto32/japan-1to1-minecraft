//! Target area: a set of 1st-order mesh cells (第1次地域区画).

use crate::store::mesh_bounds;
use anyhow::{Result, bail};

pub const DEFAULT_MESHES: &str = "5137,5138,5139,5237,5238,5239,5337,5338,5339";

#[derive(Clone)]
pub struct Area {
    pub meshes: Vec<String>,
    /// (lat_min, lon_min, lat_max, lon_max) per mesh
    rects: Vec<(f64, f64, f64, f64)>,
    pub lat_min: f64,
    pub lat_max: f64,
    pub lon_min: f64,
    pub lon_max: f64,
}

impl Area {
    pub fn new(list: &str) -> Result<Self> {
        let mut meshes = Vec::new();
        let mut rects = Vec::new();
        for m in list.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            if m.len() != 4 {
                bail!("1st mesh code must have 4 digits: {m}");
            }
            let Some((la, lo, dla, dlo)) = mesh_bounds(m) else { bail!("bad mesh {m}") };
            meshes.push(m.to_string());
            rects.push((la, lo, la + dla, lo + dlo));
        }
        if rects.is_empty() {
            bail!("empty area");
        }
        let lat_min = rects.iter().map(|r| r.0).fold(f64::MAX, f64::min);
        let lon_min = rects.iter().map(|r| r.1).fold(f64::MAX, f64::min);
        let lat_max = rects.iter().map(|r| r.2).fold(f64::MIN, f64::max);
        let lon_max = rects.iter().map(|r| r.3).fold(f64::MIN, f64::max);
        Ok(Area { meshes, rects, lat_min, lat_max, lon_min, lon_max })
    }

    #[inline]
    pub fn contains(&self, lat: f64, lon: f64) -> bool {
        // half-open [min, max) so neighbouring meshes never overlap
        self.rects.iter().any(|r| lat >= r.0 && lat < r.2 && lon >= r.1 && lon < r.3)
    }

    /// Closed boundary polylines of each mesh rectangle, sampled every `step_deg`.
    pub fn boundary_points(&self, step_deg: f64) -> Vec<(f64, f64)> {
        let mut v = Vec::new();
        for r in &self.rects {
            let n_lat = ((r.2 - r.0) / step_deg).ceil() as usize;
            let n_lon = ((r.3 - r.1) / step_deg).ceil() as usize;
            for i in 0..=n_lon {
                let lo = r.1 + (r.3 - r.1) * i as f64 / n_lon as f64;
                v.push((r.0, lo));
                v.push((r.2, lo));
            }
            for i in 0..=n_lat {
                let la = r.0 + (r.2 - r.0) * i as f64 / n_lat as f64;
                v.push((la, r.1));
                v.push((la, r.3));
            }
        }
        v
    }
}
