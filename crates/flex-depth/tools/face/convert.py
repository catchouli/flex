#!/usr/bin/env python3
"""Turns YuNet's ONNX file into models/yunet.bin, the plan and weights face.rs runs.

    convert.py face_detection_yunet_2026may.onnx ../../../../models/yunet.bin
    convert.py face_detection_yunet_2026may.onnx --reference picture.png out.rgb out.bin

The file is a little-endian u32 giving the length of a JSON plan, the plan, then the weights
as f32. The second form writes a picture and what ONNX Runtime makes of it, for the
`matches_onnx` test. Needs onnx (and onnxruntime, numpy and pillow for --reference).

The model is from https://github.com/opencv/opencv_zoo (models/face_detection_yunet), MIT.
"""
import json
import struct
import sys

import numpy as np
import onnx
from onnx import numpy_helper

WIDTH, HEIGHT = 640, 352
OUTPUTS = [f"{kind}_{stride}" for stride in (8, 16, 32) for kind in ("cls", "obj", "bbox", "kps")]


def convert(model):
    graph = model.graph
    initial = {t.name: numpy_helper.to_array(t) for t in graph.initializer}
    producer = {o: n for n in graph.node for o in n.output}
    users = {}
    for n in graph.node:
        for i in n.input:
            users.setdefault(i, []).append(n)
    attrs = lambda n: {a.name: onnx.helper.get_attribute_value(a) for a in n.attribute}
    weights = []

    def push(array):
        at = sum(len(w) for w in weights)
        weights.append(array.astype(np.float32).ravel())
        return at

    ops = []
    for n in graph.node:
        if n.op_type == "Conv":
            a = attrs(n)
            w, b = initial[n.input[1]], initial[n.input[2]]
            output, relu = n.output[0], False
            after = users.get(output, [])
            if len(after) == 1 and after[0].op_type == "Relu":
                output, relu = after[0].output[0], True
            ops.append({
                "op": "conv", "in": n.input[0], "out": output, "kernel": int(a["kernel_shape"][0]),
                "stride": int(a["strides"][0]), "groups": int(a["group"]), "cout": int(w.shape[0]),
                "cin": int(w.shape[1]) * int(a["group"]), "w": push(w), "b": push(b), "relu": relu,
            })
        elif n.op_type == "MaxPool":
            ops.append({"op": "pool", "in": n.input[0], "out": n.output[0]})
        elif n.op_type == "Add":
            resized = producer[n.input[1]]
            assert resized.op_type == "Resize", "expected Add(feature, Resize(coarser))"
            ops.append({"op": "add_up", "in": n.input[0], "up": resized.input[0], "out": n.output[0]})
        elif n.op_type not in ("Relu", "Resize", "Transpose", "Reshape", "Sigmoid"):
            raise SystemExit(f"unexpected {n.op_type}")

    def source(name):
        """The convolution behind an output, past the sigmoid and reshaping done in Rust."""
        while producer[name].op_type in ("Transpose", "Reshape", "Sigmoid"):
            name = producer[name].input[0]
        return name

    plan = {"ops": ops, "outputs": {name: source(name) for name in OUTPUTS}}
    return plan, np.concatenate(weights)


def reference(path, picture, rgb_out, out):
    import onnxruntime as ort
    from PIL import Image

    image = Image.open(picture).convert("RGB")
    k = min(WIDTH / image.width, HEIGHT / image.height)
    w, h = round(image.width * k), round(image.height * k)
    canvas = np.zeros((HEIGHT, WIDTH, 3), np.uint8)
    canvas[:h, :w] = np.asarray(image.resize((w, h), Image.BILINEAR))
    canvas.tofile(rgb_out)
    x = canvas[..., ::-1].transpose(2, 0, 1)[None].astype(np.float32)
    session = ort.InferenceSession(path, providers=["CPUExecutionProvider"])
    result = session.run(OUTPUTS, {"input": x})
    np.concatenate([r.ravel() for r in result]).astype(np.float32).tofile(out)
    print("wrote", out, [r.shape for r in result])


def main():
    if "--reference" in sys.argv:
        reference(sys.argv[1], *sys.argv[sys.argv.index("--reference") + 1:])
        return
    plan, weights = convert(onnx.load(sys.argv[1]))
    text = json.dumps(plan, separators=(",", ":")).encode()
    with open(sys.argv[2], "wb") as f:
        f.write(struct.pack("<I", len(text)) + text + weights.astype("<f4").tobytes())
    print(len(plan["ops"]), "ops,", len(weights), "weights")


main()
