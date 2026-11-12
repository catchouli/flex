#!/usr/bin/env python3
"""Turns YAMNet's Keras weights (storage.googleapis.com/audioset/yamnet.h5, from
github.com/tensorflow/models research/audioset/yamnet, Apache 2.0) into models/sound.safetensors,
the half-precision file src/sound.rs runs. Batch norms are folded into the convolutions before
them. The class names (yamnet_class_map.csv, from the same folder) go into the metadata.

    convert.py yamnet.h5 yamnet_class_map.csv ../../../../models/sound.safetensors

Tensors, for activations laid out as rows of channels:
    l1.w [32, 9] (out, then 3x3 taps), l1.b [32]
    lN.dw [C, 9], lN.dwb [C], lN.pw [in, out], lN.pwb [out]   for N = 2..14
    logits.w [1024, 521], logits.b [521]

Needs h5py, numpy and safetensors.
"""

import csv
import sys

import h5py
import numpy as np
from safetensors.numpy import save_file

EPSILON = 1e-4


def fold(f, name):
    """The scale and bias a batch norm (center, no scale) adds after `name`."""
    bn = f[f"{name}/bn/{name}/bn"]
    scale = 1.0 / np.sqrt(bn["moving_variance:0"][()] + EPSILON)
    return scale, bn["beta:0"][()] - bn["moving_mean:0"][()] * scale


def main():
    h5, class_map, out = sys.argv[1:4]
    f = h5py.File(h5, "r")
    t = {}
    k = f["layer1/conv/layer1/conv/kernel:0"][()]  # [3, 3, 1, 32]
    scale, bias = fold(f, "layer1/conv")
    t["l1.w"] = (k[:, :, 0, :].reshape(9, -1).T * scale[:, None])
    t["l1.b"] = bias
    for n in range(2, 15):
        p = f"layer{n}"
        dw = f[f"{p}/depthwise_conv/{p}/depthwise_conv/depthwise_kernel:0"][()]  # [3, 3, C, 1]
        scale, bias = fold(f, f"{p}/depthwise_conv")
        t[f"l{n}.dw"] = dw[:, :, :, 0].reshape(9, -1).T * scale[:, None]
        t[f"l{n}.dwb"] = bias
        pw = f[f"{p}/pointwise_conv/{p}/pointwise_conv/kernel:0"][()]  # [1, 1, in, out]
        scale, bias = fold(f, f"{p}/pointwise_conv")
        t[f"l{n}.pw"] = pw[0, 0] * scale[None, :]
        t[f"l{n}.pwb"] = bias
    t["logits.w"] = f["logits/logits/kernel:0"][()]
    t["logits.b"] = f["logits/logits/bias:0"][()]
    with open(class_map) as c:
        rows = list(csv.reader(c))[1:]
    names = [r[2] for r in rows]
    assert len(names) == t["logits.b"].shape[0]
    t = {name: np.ascontiguousarray(v, dtype=np.float16) for name, v in t.items()}
    save_file(t, out, {"classes": "\n".join(names)})
    print(f"{sum(v.size for v in t.values())} weights, {len(names)} classes")


if __name__ == "__main__":
    main()
