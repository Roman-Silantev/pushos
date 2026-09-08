#!/usr/bin/env python3
"""Draws the PushOS Studio icon.

Kept as a script rather than a binary blob so the icon can be changed without
opening a graphics application, and so its provenance is obvious.

    python3 icons/generate.py icons/icon.png
    npx tauri icon icons/icon.png

Writes a square RGBA PNG using only the standard library.
"""

import struct
import sys
import zlib

SIZE = 1024
# The same palette the display uses, so Studio and the panel look related.
GROUND = (14, 15, 19)
PAD_DIM = (38, 41, 50)
PAD_LIT = (88, 166, 255)
PAD_WARM = (255, 176, 0)

# Which cells of the 4x4 motif are lit. A diagonal reads as a grid rather than
# as a logo of something else.
LIT = {(0, 0), (1, 1), (2, 2), (3, 3), (0, 3)}
WARM = {(3, 0)}


def rounded(x, y, left, top, size, radius):
    """Whether a point falls inside a rounded square."""
    right, bottom = left + size, top + size
    if not (left <= x < right and top <= y < bottom):
        return False

    for corner_x, corner_y in (
        (left + radius, top + radius),
        (right - radius, top + radius),
        (left + radius, bottom - radius),
        (right - radius, bottom - radius),
    ):
        inside_x = x < left + radius or x >= right - radius
        inside_y = y < top + radius or y >= bottom - radius
        if inside_x and inside_y:
            near = (x - corner_x) ** 2 + (y - corner_y) ** 2 <= radius**2
            if abs(x - corner_x) <= radius and abs(y - corner_y) <= radius:
                return near
    return True


def render():
    margin = SIZE // 8
    body = SIZE - margin * 2
    gap = body // 24
    cell = (body - gap * 3) // 4

    rows = []
    for y in range(SIZE):
        row = bytearray()
        for x in range(SIZE):
            colour, alpha = (0, 0, 0), 0

            if rounded(x, y, 0, 0, SIZE, SIZE // 5):
                colour, alpha = GROUND, 255

                for grid_y in range(4):
                    for grid_x in range(4):
                        left = margin + grid_x * (cell + gap)
                        top = margin + grid_y * (cell + gap)
                        if rounded(x, y, left, top, cell, cell // 4):
                            if (grid_x, grid_y) in WARM:
                                colour = PAD_WARM
                            elif (grid_x, grid_y) in LIT:
                                colour = PAD_LIT
                            else:
                                colour = PAD_DIM

            row += bytes((*colour, alpha))
        rows.append(bytes(row))
    return rows


def write_png(path, rows):
    raw = b"".join(b"\x00" + row for row in rows)

    def chunk(kind, payload):
        head = struct.pack(">I", len(payload)) + kind
        return head + payload + struct.pack(">I", zlib.crc32(kind + payload))

    header = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    with open(path, "wb") as out:
        out.write(b"\x89PNG\r\n\x1a\n")
        out.write(chunk(b"IHDR", header))
        out.write(chunk(b"IDAT", zlib.compress(raw, 9)))
        out.write(chunk(b"IEND", b""))


if __name__ == "__main__":
    target = sys.argv[1] if len(sys.argv) > 1 else "icon.png"
    write_png(target, render())
    print(f"wrote {target}")
