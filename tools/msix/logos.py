"""DSK-13: Microsoft Store logos for the MSIX package.

Draws the same picture as `app_icon()` in prswt-desktop (a rounded blue tile
with three Gantt bars, the last one critical red) at the sizes the manifest
names, so the repo ships no image files. Standard library only.

    python tools/msix/logos.py OUT_DIR
"""

import struct
import sys
import zlib
from pathlib import Path

# Geometry on the 64x64 grid of app_icon(): corner radius, tile colour, bars
# as (top, x0, x1, colour), each 8 units tall.
GRID = 64
RADIUS = 12
TILE = (36, 92, 196)
BARS = [
    (16, 12, 36, (255, 255, 255)),
    (29, 24, 48, (255, 255, 255)),
    (42, 34, 54, (235, 80, 70)),
]
# Supersampling per axis, for smooth edges at every size.
SS = 4
# File name -> (width, height); the tile is centred in wide logos.
LOGOS = {
    "Square44x44Logo.png": (44, 44),
    "Square150x150Logo.png": (150, 150),
    "StoreLogo.png": (50, 50),
    "Wide310x150Logo.png": (310, 150),
}


def main() -> None:
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    for name, (w, h) in LOGOS.items():
        (out / name).write_bytes(png(w, h, render(w, h)))


def render(w: int, h: int) -> list[bytes]:
    """RGBA rows; the tile is a square of side min(w, h), centred."""
    side = min(w, h)
    ox, oy = (w - side) / 2, (h - side) / 2
    rows = []
    for y in range(h):
        row = bytearray()
        for x in range(w):
            acc = [0.0, 0.0, 0.0, 0.0]
            for sy in range(SS):
                for sx in range(SS):
                    u = (x - ox + (sx + 0.5) / SS) * GRID / side
                    v = (y - oy + (sy + 0.5) / SS) * GRID / side
                    c = sample(u, v)
                    if c:
                        for i in range(3):
                            acc[i] += c[i]
                        acc[3] += 1
            n = acc[3]
            if n:
                row += bytes(round(acc[i] / n) for i in range(3))
                row.append(round(255 * n / (SS * SS)))
            else:
                row += b"\0\0\0\0"
        rows.append(bytes(row))
    return rows


def sample(u: float, v: float):
    """Colour at grid point (u, v), or None outside the rounded tile."""
    if not (0 <= u < GRID and 0 <= v < GRID):
        return None
    cx = min(max(u, RADIUS), GRID - RADIUS)
    cy = min(max(v, RADIUS), GRID - RADIUS)
    if (u - cx) ** 2 + (v - cy) ** 2 > RADIUS**2:
        return None
    for top, x0, x1, colour in BARS:
        if top <= v < top + 8 and x0 <= u < x1:
            return colour
    return TILE


def png(w: int, h: int, rows: list[bytes]) -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    raw = b"".join(b"\0" + r for r in rows)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


if __name__ == "__main__":
    main()
