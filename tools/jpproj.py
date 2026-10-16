"""The world's map projection in Python (same as src/proj.rs and docs/js/proj.js):
Lambert Conformal Conic 2SP on GRS80, standard parallels 30/40N, origin
36N 138.5E, false easting/northing 0. Minecraft X = easting, Z = -northing,
1 block = 1 m. A ground surface at elevation h m is the block Y = -1760 + round(h).
"""
import math

A = 6378137.0
F = 1 / 298.257222101
E2 = F * (2 - F)
E = math.sqrt(E2)
LAT1, LAT2, LAT0, LON0 = map(math.radians, (30.0, 40.0, 36.0, 138.5))
SEA_Y = -1760


def _m(p):
    return math.cos(p) / math.sqrt(1 - E2 * math.sin(p) ** 2)


def _t(p):
    s = math.sin(p)
    return math.tan(math.pi / 4 - p / 2) / ((1 - E * s) / (1 + E * s)) ** (E / 2)


N = (math.log(_m(LAT1)) - math.log(_m(LAT2))) / (math.log(_t(LAT1)) - math.log(_t(LAT2)))
AF = A * _m(LAT1) / (N * _t(LAT1) ** N)
RHO0 = AF * _t(LAT0) ** N


def ll_to_mc(lat, lon):
    """(lat, lon) degrees -> Minecraft (X, Z) as floats."""
    rho = AF * _t(math.radians(lat)) ** N
    th = N * (math.radians(lon) - LON0)
    return rho * math.sin(th), -(RHO0 - rho * math.cos(th))


def mc_to_ll(x, z):
    """Minecraft (X, Z) -> (lat, lon) degrees."""
    y = -z
    dy = RHO0 - y
    rho = math.copysign(math.hypot(x, dy), N)
    th = math.atan2(x, dy)
    t = (rho / AF) ** (1 / N)
    p = math.pi / 2 - 2 * math.atan(t)
    for _ in range(15):
        s = math.sin(p)
        p = math.pi / 2 - 2 * math.atan(t * ((1 - E * s) / (1 + E * s)) ** (E / 2))
    return math.degrees(p), math.degrees(th / N + LON0)


if __name__ == "__main__":
    import sys

    if len(sys.argv) == 3:
        la, lo = float(sys.argv[1]), float(sys.argv[2])
        x, z = ll_to_mc(la, lo)
        print(f"X={x:.2f} Z={z:.2f}")
