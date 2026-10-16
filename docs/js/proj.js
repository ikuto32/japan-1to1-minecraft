// The world's map projection (same as src/proj.rs and tools/jpproj.py):
// Lambert Conformal Conic 2SP on GRS80, standard parallels 30°N / 40°N,
// origin 36°N 138.5°E. Minecraft X = easting, Z = -northing, 1 block = 1 m.
// Ground at elevation h m is the block Y = -1760 + round(h).
"use strict";
const JPProj = (() => {
  const A = 6378137.0;
  const F = 1 / 298.257222101;
  const E2 = F * (2 - F);
  const E = Math.sqrt(E2);
  const rad = (d) => (d * Math.PI) / 180;
  const deg = (r) => (r * 180) / Math.PI;
  const LAT1 = rad(30), LAT2 = rad(40), LAT0 = rad(36), LON0 = rad(138.5);
  const m = (p) => Math.cos(p) / Math.sqrt(1 - E2 * Math.sin(p) ** 2);
  const t = (p) => {
    const s = Math.sin(p);
    return Math.tan(Math.PI / 4 - p / 2) / Math.pow((1 - E * s) / (1 + E * s), E / 2);
  };
  const N = (Math.log(m(LAT1)) - Math.log(m(LAT2))) / (Math.log(t(LAT1)) - Math.log(t(LAT2)));
  const AF = (A * m(LAT1)) / (N * Math.pow(t(LAT1), N));
  const RHO0 = AF * Math.pow(t(LAT0), N);

  /** (lat, lon) in degrees -> [X, Z] Minecraft coordinates (floats). */
  function llToMc(lat, lon) {
    const rho = AF * Math.pow(t(rad(lat)), N);
    const th = N * (rad(lon) - LON0);
    return [rho * Math.sin(th), -(RHO0 - rho * Math.cos(th))];
  }

  /** Minecraft [X, Z] -> [lat, lon] in degrees. */
  function mcToLl(x, z) {
    const y = -z;
    const dy = RHO0 - y;
    const rho = Math.sign(N) * Math.hypot(x, dy);
    const th = Math.atan2(x, dy);
    const tt = Math.pow(rho / AF, 1 / N);
    let p = Math.PI / 2 - 2 * Math.atan(tt);
    for (let i = 0; i < 15; i++) {
      const s = Math.sin(p);
      p = Math.PI / 2 - 2 * Math.atan(tt * Math.pow((1 - E * s) / (1 + E * s), E / 2));
    }
    return [deg(p), deg(th / N + LON0)];
  }

  return { llToMc, mcToLl, SEA_Y: -1760, TOP_Y: 2031, TILE: 10240, AREA: { latMin: 34, latMax: 36, lonMin: 137, lonMax: 140 } };
})();
