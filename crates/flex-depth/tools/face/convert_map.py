#!/usr/bin/env python3
"""Turns MediaPipe's canonical face model into src/face_map.bin, the triangles face_map.rs draws.

    convert_map.py canonical_face_model.obj ../../../flex-app/src/face_map.bin

The file holds one record per triangle corner, three corners a triangle: the mesh point (u16),
the part of the face (u16: see PARTS), and where it is on the face, u and v (f32), all
little-endian. A triangle's part comes from which outline its middle falls inside.

The model is mediapipe/modules/face_geometry/data/canonical_face_model.obj, Apache 2.0.
"""
import struct
import sys

SKIN, LIPS, NOSE, BROWS, EYE, MOUTH = range(6)
# First match wins, so the mouth comes before the lips around it.
PARTS = [
    (MOUTH, [78, 191, 80, 81, 82, 13, 312, 311, 310, 415, 308, 324, 318, 402, 317, 14, 87, 178, 88, 95]),
    (LIPS, [61, 146, 91, 181, 84, 17, 314, 405, 321, 375, 291, 409, 270, 269, 267, 0, 37, 39, 40, 185]),
    (EYE, [33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246]),
    (EYE, [263, 249, 390, 373, 374, 380, 381, 382, 362, 398, 384, 385, 386, 387, 388, 466]),
    (BROWS, [70, 63, 105, 66, 107, 55, 65, 52, 53, 46]),
    (BROWS, [300, 293, 334, 296, 336, 285, 295, 282, 283, 276]),
    (NOSE, [168, 440, 344, 278, 294, 327, 326, 2, 97, 98, 64, 48, 115, 220]),
]


def inside(point, outline):
    x, y = point
    found = False
    for (ax, ay), (bx, by) in zip(outline, outline[1:] + outline[:1]):
        if (ay > y) != (by > y) and x < (bx - ax) * (y - ay) / (by - ay) + ax:
            found = not found
    return found


def main():
    vertices, textures, faces = [], [], []
    for line in open(sys.argv[1]):
        kind, *rest = line.split() or [""]
        if kind == "v":
            vertices.append(tuple(float(v) for v in rest[:2]))
        elif kind == "vt":
            textures.append(tuple(float(v) for v in rest[:2]))
        elif kind == "f":
            faces.append([tuple(int(i) - 1 for i in corner.split("/")[:2]) for corner in rest])
    outlines = [(part, [vertices[i] for i in ring]) for part, ring in PARTS]
    counts = [0] * 6
    with open(sys.argv[2], "wb") as out:
        for corners in faces:
            middle = tuple(sum(vertices[v][axis] for v, _ in corners) / 3 for axis in (0, 1))
            part = next((p for p, outline in outlines if inside(middle, outline)), SKIN)
            counts[part] += 1
            for v, t in corners:
                out.write(struct.pack("<HHff", v, part, *textures[t]))
    print(len(faces), "triangles; skin, lips, nose, brows, eyes, mouth:", counts)


main()
