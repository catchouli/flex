"""Reference output for flex-depth's `track_matches_python` test: finds corners in a picture and
follows them to the next, as track.rs does, and saves the lot.

    python reference.py <picture> <next picture> <out.safetensors>

Needs numpy, pillow and safetensors.
"""

import sys

import numpy as np
from PIL import Image
from safetensors.numpy import save_file

LEVELS, RADIUS, STEPS, CLOSE_ENOUGH = 4, 7, 9, 0.02
CELL, CORNER, EDGE, CORNER_RADIUS = 14, 4e-4, 10, 3


def box(a, r):
    integral = np.pad(a.cumsum(0).cumsum(1), ((1, 0), (1, 0)))
    h, w = a.shape
    y0, y1 = np.clip(np.arange(h) - r, 0, h), np.clip(np.arange(h) + r + 1, 0, h)
    x0, x1 = np.clip(np.arange(w) - r, 0, w), np.clip(np.arange(w) + r + 1, 0, w)
    total = integral[y1][:, x1] - integral[y0][:, x1] - integral[y1][:, x0] + integral[y0][:, x0]
    return total / ((y1 - y0)[:, None] * (x1 - x0)[None])


def corners(g):
    iy, ix = np.gradient(g)
    a, b, c = box(ix * ix, CORNER_RADIUS), box(ix * iy, CORNER_RADIUS), box(iy * iy, CORNER_RADIUS)
    strength = (a + c - np.sqrt((a - c) ** 2 + 4 * b * b)) / 2
    strength[:EDGE] = strength[-EDGE:] = 0
    strength[:, :EDGE] = strength[:, -EDGE:] = 0
    h, w = g.shape
    out = []
    for j in range(h // CELL):
        for i in range(w // CELL):
            cell = strength[j * CELL : (j + 1) * CELL, i * CELL : (i + 1) * CELL]
            k = cell.argmax()
            if cell.flat[k] > CORNER:
                y, x = np.unravel_index(k, cell.shape)
                out.append((x + i * CELL, y + j * CELL))
    return np.array(out, np.float32).reshape(-1, 2)


def pyramid(g):
    levels = [g]
    for _ in range(LEVELS - 1):
        a = levels[-1]
        h, w = a.shape[0] // 2 * 2, a.shape[1] // 2 * 2
        a = a[:h, :w]
        levels.append((a[::2, ::2] + a[1::2, ::2] + a[::2, 1::2] + a[1::2, 1::2]) / 4)
    return [(a, *np.gradient(a)[::-1]) for a in levels]


def sample(image, x, y):
    h, w = image.shape
    x0, y0 = np.clip(np.floor(x).astype(np.int32), 0, w - 2), np.clip(np.floor(y).astype(np.int32), 0, h - 2)
    fx, fy = np.clip(x - x0, 0, 1), np.clip(y - y0, 0, 1)
    return (image[y0, x0] * (1 - fx) + image[y0, x0 + 1] * fx) * (1 - fy) + (image[y0 + 1, x0] * (1 - fx) + image[y0 + 1, x0 + 1] * fx) * fy


OFFSETS = np.arange(-RADIUS, RADIUS + 1, dtype=np.float32)
OY, OX = [a.ravel() for a in np.meshgrid(OFFSETS, OFFSETS, indexing="ij")]


def follow(p0, p1, points, guess):
    v = guess.astype(np.float32) / 2 ** (LEVELS - 1)
    left = np.zeros(len(points), np.float32)
    for level in range(LEVELS - 1, -1, -1):
        (a, ax, ay), (b, _, _) = p0[level], p1[level]
        k = 2.0**-level
        x, y = (points[:, 0] * k)[:, None] + OX, (points[:, 1] * k)[:, None] + OY
        t = sample(a, x, y)
        ix, iy = sample(ax, x, y), sample(ay, x, y)
        t = t - t.mean(1, keepdims=True)
        aa, bb, cc = (ix * ix).sum(1), (ix * iy).sum(1), (iy * iy).sum(1)
        d = aa * cc - bb * bb + 1e-9
        going = np.ones(len(points), bool)
        for _ in range(STEPS):
            if not going.any():
                break
            j = sample(b, x + v[:, 0, None], y + v[:, 1, None])
            e = t - (j - j.mean(1, keepdims=True))
            bx, by = (e * ix).sum(1), (e * iy).sum(1)
            du, dv = (cc * bx - bb * by) / d, (aa * by - bb * bx) / d
            left[going] = np.abs(e).mean(1)[going]
            v[going, 0] += du[going]
            v[going, 1] += dv[going]
            going &= np.maximum(np.abs(du), np.abs(dv)) >= CLOSE_ENOUGH
        if level:
            v *= 2
    return points + v, left


def main():
    a, b = [np.asarray(Image.open(p).convert("L")).astype(np.float32) / 255 for p in sys.argv[1:3]]
    found = corners(a)
    p0, p1 = pyramid(a), pyramid(b)
    there, left = follow(p0, p1, found, np.zeros_like(found))
    back, _ = follow(p1, p0, there, found - there)
    save_file({k: np.asarray(v, np.float32) for k, v in dict(a=a, b=b, corners=found, points=found, there=there, back=back, left=left).items()}, sys.argv[3])
    print(len(found), "points")


if __name__ == "__main__":
    main()
