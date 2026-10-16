//! Lambert Conformal Conic (2SP) on GRS80 — the fixed project-wide CRS.
//! Standard parallels 30°N / 40°N, origin 36°N 138°30'E, false E/N = 0.
//! Minecraft X = easting, Z = -northing.

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

pub const A: f64 = 6378137.0;
pub const INV_F: f64 = 298.257222101;
pub const LAT1: f64 = 30.0;
pub const LAT2: f64 = 40.0;
pub const LAT0: f64 = 36.0;
pub const LON0: f64 = 138.5;

pub struct Lcc {
    e: f64,
    n: f64,
    af: f64, // a * F
    rho0: f64,
    lon0: f64,
}

impl Lcc {
    pub fn new() -> Self {
        let f = 1.0 / INV_F;
        let e2 = f * (2.0 - f);
        let e = e2.sqrt();
        let m = |p: f64| p.cos() / (1.0 - e2 * p.sin().powi(2)).sqrt();
        let t = |p: f64| (FRAC_PI_4 - p / 2.0).tan() / ((1.0 - e * p.sin()) / (1.0 + e * p.sin())).powf(e / 2.0);
        let (p1, p2, p0) = (LAT1.to_radians(), LAT2.to_radians(), LAT0.to_radians());
        let n = (m(p1).ln() - m(p2).ln()) / (t(p1).ln() - t(p2).ln());
        let ff = m(p1) / (n * t(p1).powf(n));
        let af = A * ff;
        let rho0 = af * t(p0).powf(n);
        Lcc { e, n, af, rho0, lon0: LON0.to_radians() }
    }

    fn t(&self, p: f64) -> f64 {
        let s = p.sin();
        (FRAC_PI_4 - p / 2.0).tan() / ((1.0 - self.e * s) / (1.0 + self.e * s)).powf(self.e / 2.0)
    }

    /// (lat, lon) degrees -> (easting, northing) metres.
    pub fn forward(&self, lat: f64, lon: f64) -> (f64, f64) {
        let rho = self.af * self.t(lat.to_radians()).powf(self.n);
        let th = self.n * (lon.to_radians() - self.lon0);
        (rho * th.sin(), self.rho0 - rho * th.cos())
    }

    /// (easting, northing) metres -> (lat, lon) degrees.
    pub fn inverse(&self, x: f64, y: f64) -> (f64, f64) {
        let dy = self.rho0 - y;
        let rho = (x * x + dy * dy).sqrt().copysign(self.n);
        let th = x.atan2(dy);
        let t = (rho / self.af).powf(1.0 / self.n);
        let mut p = FRAC_PI_2 - 2.0 * t.atan();
        for _ in 0..15 {
            let s = p.sin();
            let np = FRAC_PI_2 - 2.0 * (t * ((1.0 - self.e * s) / (1.0 + self.e * s)).powf(self.e / 2.0)).atan();
            if (np - p).abs() < 1e-14 {
                p = np;
                break;
            }
            p = np;
        }
        (p.to_degrees(), (th / self.n + self.lon0).to_degrees())
    }

    /// Minecraft block (X,Z) centre -> (lat, lon).
    pub fn mc_to_ll(&self, x: f64, z: f64) -> (f64, f64) {
        self.inverse(x, -z)
    }

    /// (lat, lon) -> Minecraft continuous coords (X, Z).
    pub fn ll_to_mc(&self, lat: f64, lon: f64) -> (f64, f64) {
        let (e, n) = self.forward(lat, lon);
        (e, -n)
    }
}

/// Bilinear lat/lon lookup for a rectangular block of Minecraft columns,
/// computed exactly on a coarse lattice (every `STEP` blocks) and interpolated.
/// The projection is smooth enough that the error is < 1e-9 deg (~0.1 mm).
pub struct LlGrid {
    pub x0: i64,
    pub z0: i64,
    pub nx: usize, // lattice nodes per row
    lat: Vec<f64>,
    lon: Vec<f64>,
}

pub const STEP: i64 = 32;

impl LlGrid {
    /// Covers block columns x0..x0+w, z0..z0+h (centres at +0.5).
    pub fn new(p: &Lcc, x0: i64, z0: i64, w: i64, h: i64) -> Self {
        let nx = (w / STEP + 2) as usize;
        let nz = (h / STEP + 2) as usize;
        let mut lat = vec![0.0; nx * nz];
        let mut lon = vec![0.0; nx * nz];
        for j in 0..nz {
            for i in 0..nx {
                let (la, lo) = p.mc_to_ll((x0 + i as i64 * STEP) as f64 + 0.5, (z0 + j as i64 * STEP) as f64 + 0.5);
                lat[j * nx + i] = la;
                lon[j * nx + i] = lo;
            }
        }
        LlGrid { x0, z0, nx, lat, lon }
    }

    /// lat/lon of block centre (x, z) (absolute MC coords within the grid).
    #[inline]
    pub fn get(&self, x: i64, z: i64) -> (f64, f64) {
        let dx = x - self.x0;
        let dz = z - self.z0;
        let i = (dx / STEP) as usize;
        let j = (dz / STEP) as usize;
        let fx = (dx % STEP) as f64 / STEP as f64;
        let fz = (dz % STEP) as f64 / STEP as f64;
        let k = j * self.nx + i;
        let bl = |a: &Vec<f64>| {
            let t = a[k] + (a[k + 1] - a[k]) * fx;
            let b = a[k + self.nx] + (a[k + self.nx + 1] - a[k + self.nx]) * fx;
            t + (b - t) * fz
        };
        (bl(&self.lat), bl(&self.lon))
    }
}

/// Japan Plane Rectangular CS (Gauss–Krüger, m0 = 0.9999) inverse, after
/// Kawase (2011) as used by GSI. `x_east`/`y_north` in metres.
pub struct Tm {
    lam0: f64,
    abar: f64,
    sphi0: f64,
    beta: [f64; 5],
    delta: [f64; 6],
}

impl Tm {
    /// Zone VIII (origin 36°N 138°30'E): Shizuoka, Yamanashi, Niigata, Nagano.
    pub fn zone8() -> Self {
        Tm::new(36.0, 138.5)
    }

    pub fn new(lat0: f64, lon0: f64) -> Self {
        let n = 1.0 / (2.0 * INV_F - 1.0);
        let m0 = 0.9999;
        let a0 = 1.0 + n * n / 4.0 + n.powi(4) / 64.0;
        let aj = [
            -1.5 * (n - n.powi(3) / 8.0 - n.powi(5) / 64.0),
            15.0 / 16.0 * (n * n - n.powi(4) / 4.0),
            -35.0 / 48.0 * (n.powi(3) - 5.0 / 16.0 * n.powi(5)),
            315.0 / 512.0 * n.powi(4),
            -693.0 / 1280.0 * n.powi(5),
        ];
        let beta = [
            n / 2.0 - 2.0 * n * n / 3.0 + 37.0 * n.powi(3) / 96.0 - n.powi(4) / 360.0 - 81.0 * n.powi(5) / 512.0,
            n * n / 48.0 + n.powi(3) / 15.0 - 437.0 * n.powi(4) / 1440.0 + 46.0 * n.powi(5) / 105.0,
            17.0 * n.powi(3) / 480.0 - 37.0 * n.powi(4) / 840.0 - 209.0 * n.powi(5) / 4480.0,
            4397.0 * n.powi(4) / 161280.0 - 11.0 * n.powi(5) / 504.0,
            4583.0 * n.powi(5) / 161280.0,
        ];
        let delta = [
            2.0 * n - 2.0 * n * n / 3.0 - 2.0 * n.powi(3) + 116.0 * n.powi(4) / 45.0 + 26.0 * n.powi(5) / 45.0 - 2854.0 * n.powi(6) / 675.0,
            7.0 * n * n / 3.0 - 8.0 * n.powi(3) / 5.0 - 227.0 * n.powi(4) / 45.0 + 2704.0 * n.powi(5) / 315.0 + 2323.0 * n.powi(6) / 945.0,
            56.0 * n.powi(3) / 15.0 - 136.0 * n.powi(4) / 35.0 - 1262.0 * n.powi(5) / 105.0 + 73814.0 * n.powi(6) / 2835.0,
            4279.0 * n.powi(4) / 630.0 - 332.0 * n.powi(5) / 35.0 - 399572.0 * n.powi(6) / 14175.0,
            4174.0 * n.powi(5) / 315.0 - 144838.0 * n.powi(6) / 6237.0,
            601676.0 * n.powi(6) / 22275.0,
        ];
        let phi0 = lat0.to_radians();
        let abar = m0 * A / (1.0 + n) * a0;
        let mut s = a0 * phi0;
        for (j, c) in aj.iter().enumerate() {
            s += c * (2.0 * (j + 1) as f64 * phi0).sin();
        }
        Tm { lam0: lon0.to_radians(), abar, sphi0: s * m0 * A / (1.0 + n), beta, delta }
    }

    /// (easting, northing) metres -> (lat, lon) degrees.
    pub fn inverse(&self, x_east: f64, y_north: f64) -> (f64, f64) {
        let xi = (y_north + self.sphi0) / self.abar;
        let eta = x_east / self.abar;
        let (mut xp, mut ep) = (xi, eta);
        for (j, b) in self.beta.iter().enumerate() {
            let k = 2.0 * (j + 1) as f64;
            xp -= b * (k * xi).sin() * (k * eta).cosh();
            ep -= b * (k * xi).cos() * (k * eta).sinh();
        }
        let chi = (xp.sin() / ep.cosh()).asin();
        let mut phi = chi;
        for (j, d) in self.delta.iter().enumerate() {
            phi += d * (2.0 * (j + 1) as f64 * chi).sin();
        }
        let lam = self.lam0 + (ep.sinh() / xp.cos()).atan();
        (phi.to_degrees(), lam.to_degrees())
    }
}
