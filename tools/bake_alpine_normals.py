"""Bake tileable tangent-space slopes and linear mip chains from the alpine DDS albedos."""

from pathlib import Path
import struct

import numpy as np
from PIL import Image


def blur_periodic(height, sigma):
    offsets = np.arange(-int(3 * sigma), int(3 * sigma) + 1)
    weights = np.exp(-0.5 * (offsets / sigma) ** 2)
    weights /= weights.sum()
    for axis in (0, 1):
        height = sum(w * np.roll(height, int(i), axis) for i, w in zip(offsets, weights))
    return height


def bake(path):
    dds = path.read_bytes()
    side = struct.unpack_from("<I", dds, 16)[0]
    rgba = Image.frombytes("RGBA", (side, side), dds[148:], "bcn", (1, "DXT1"))
    height = np.asarray(rgba, dtype=float)[:, :, :3] @ [0.22, 0.70, 0.08]
    # Smooth BC1 block boundaries and retain the wider bedding and stone edges.
    relief = sum(weight * blur_periodic(height, sigma) for weight, sigma in
                 [(0.55, 1.2), (1.7, 4.0), (3.2, 12.0)])
    slopes = np.stack([(np.roll(relief, -1, axis) - np.roll(relief, 1, axis)) * 0.5
                       for axis in (1, 0)], axis=-1)
    slopes *= 0.85 / np.percentile(np.abs(slopes), 98.5)
    slopes = np.clip(slopes, -1, 1)

    header = bytearray(dds[:148])
    struct.pack_into("<I", header, 8, 0x2100F)  # DDSD_PITCH and a full mip chain
    struct.pack_into("<I", header, 20, 2 * side)
    struct.pack_into("<I", header, 128, 51)  # DXGI_FORMAT_R8G8_SNORM
    data = bytearray(header)
    while True:
        data.extend(np.rint(127 * slopes).astype(np.int8).tobytes())
        if len(slopes) == 1:
            break
        size = len(slopes) // 2
        slopes = slopes.reshape(size, 2, size, 2, 2).mean(axis=(1, 3))
    path.with_stem(path.stem + "-slopes").write_bytes(data)


if __name__ == "__main__":
    terrain = Path(__file__).resolve().parents[1] / "assets" / "terrain"
    for material in ["weathered-limestone", "laminated-shale", "coarse-scree"]:
        bake(terrain / (material + ".dds"))
