"""Bake the wrapping 64³ RGBA8 cloud noise volume. Requires NumPy."""
from itertools import product
from pathlib import Path
import numpy as np

rng = np.random.default_rng(4157)
points = (np.indices((64, 64, 64), dtype=np.float32).transpose(1, 2, 3, 0) + 0.5) / 64


def worley(cells):
    features = rng.random((cells, cells, cells, 3))
    lattice = np.floor(points * cells).astype(int)
    local = points * cells - lattice
    nearest = np.full(points.shape[:-1], 3.0)
    for offset in product(range(-1, 2), repeat=3):
        indices = tuple(((lattice + offset) % cells).transpose(3, 0, 1, 2))
        delta = offset + features[indices] - local
        nearest = np.minimum(nearest, np.sum(delta * delta, axis=-1))
    return 1 - np.sqrt(nearest)


def perlin(cells):
    gradients = rng.normal(size=(cells, cells, cells, 3))
    gradients /= np.linalg.norm(gradients, axis=-1, keepdims=True)
    lattice = np.floor(points * cells).astype(int)
    local = points * cells - lattice
    fade = local**3 * (local * (6 * local - 15) + 10)
    result = np.zeros(points.shape[:-1])
    for corner in product(range(2), repeat=3):
        indices = tuple(((lattice + corner) % cells).transpose(3, 0, 1, 2))
        weight = np.prod(np.where(corner, fade, 1 - fade), axis=-1)
        result += weight * np.sum(gradients[indices] * (local - corner), axis=-1)
    return 0.5 + result


shape = 0.68 * perlin(4) + 0.32 * worley(5)
detail = 0.65 * worley(12) + 0.35 * worley(24)
volume = np.stack([shape, detail, perlin(3), worley(8)], axis=-1)
out = Path(__file__).resolve().parents[1] / "assets" / "cloud-noise.rgba8"
np.rint(volume.clip(0, 1) * 255).astype("uint8").tofile(out)
