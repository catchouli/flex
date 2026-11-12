"""The built-in painting styles' numbers for Flex's "Any Painting" effect: each picture's style
from Magenta's style network (as reference.py prepares it: cropped to a square in the middle,
256 x 256, smoothed), written one after another as little-endian f32s, `magenta::STYLE` (768)
each, in the order given.

    python styles.py <saved_model_dir> <out.bin> <picture> [picture ...]

Needs tensorflow, safetensors, pillow and numpy (reference.py's).
"""

import os
import sys

import numpy as np
import tensorflow as tf

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from reference import flatten, style_picture  # noqa: E402


def main():
    model, out, pictures = sys.argv[1], sys.argv[2], sys.argv[3:]
    flat = flatten(model)
    prepared = [style_picture(p) for p in pictures]
    g = tf.Graph()
    with g.as_default():
        tf.compat.v1.import_graph_def(flat, name="")
    styles = []
    with tf.compat.v1.Session(graph=g) as sess:
        for p, picture in zip(pictures, prepared):
            s = sess.run("bottleneck/Mean:0", {"placeholder_1:0": picture}).reshape(-1)
            assert s.size == 768
            styles.append(s.astype("<f4"))
            print(p, float(s.mean()))
    np.concatenate(styles).tofile(out)


if __name__ == "__main__":
    main()
