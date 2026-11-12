#!/usr/bin/env python3
"""Turns the two networks that look closer at a face into the files face.rs runs.

    convert_detail.py mesh face_landmarks_detector.tflite ../../../../models/face_mesh.bin
    convert_detail.py head mobilenetv2.onnx ../../../../models/head_angle.bin
    convert_detail.py mesh face_landmarks_detector.tflite --reference picture.png out.rgb out.bin
    convert_detail.py head mobilenetv2.onnx --reference picture.png out.rgb out.bin

Each file is a little-endian u32 giving the length of a JSON plan, the plan, then the weights
as f16. The --reference form writes a square of a picture and what the original network makes
of it, for the `mesh_matches_tflite` and `head_matches_onnx` tests. The mesh side needs
ai-edge-litert, the head side onnx and onnxruntime; both need numpy and pillow.

The mesh is face_landmarks_detector.tflite from MediaPipe's face_landmarker.task, Apache 2.0.
The head angle is mobilenetv2.onnx from https://github.com/yakhyo/head-pose-estimation (as
used by UniFace), MIT.
"""
import json
import struct
import sys

import numpy as np

MESH_SIZE, HEAD_SIZE = 256, 224
MEAN, STD = np.array([0.485, 0.456, 0.406], np.float32), np.array([0.229, 0.224, 0.225], np.float32)


class Weights:
    def __init__(self):
        self.parts, self.size = [], 0

    def push(self, array):
        at = self.size
        self.parts.append(array.astype(np.float16).ravel())
        self.size += self.parts[-1].size
        return at

    def all(self):
        return np.concatenate(self.parts)


def mesh_interpreter(path):
    from ai_edge_litert.interpreter import Interpreter

    it = Interpreter(path, experimental_preserve_all_tensors=True)
    it.allocate_tensors()
    return it


def mesh(path):
    it = mesh_interpreter(path)
    ops = [o for o in it._get_ops_details() if o["op_name"] != "DELEGATE"]
    shape = {t["index"]: t["shape"].tolist() for t in it.get_tensor_details()}
    producer = {int(i): o for o in ops for i in o["outputs"]}
    users = {}
    for o in ops:
        for i in o["inputs"]:
            users.setdefault(int(i), []).append(o)
    input_at = int(it.get_input_details()[0]["index"])
    tensor = lambda i: "input" if i == input_at else str(i)

    def constant(i):
        o = producer.get(i)
        return it.get_tensor(int(o["inputs"][0])) if o is not None and o["op_name"] == "DEQUANTIZE" else it.get_tensor(i)

    weights, plan = Weights(), []
    for o in ops:
        name = o["op_name"]
        if name in ("CONV_2D", "DEPTHWISE_CONV_2D"):
            at, w, b = [int(i) for i in o["inputs"]]
            out = int(o["outputs"][0])
            _, h_in, _, cin = shape[at]
            _, h_out, _, cout = shape[out]
            kernel = shape[w][1]
            assert shape[w][1] == shape[w][2], "square kernels only"
            stride = h_in // h_out
            # SAME padding where it applies, which puts the odd tap after the picture.
            padded = max(kernel - stride, 0) // 2
            if name == "CONV_2D":
                groups, array = 1, constant(w).transpose(0, 3, 1, 2)
            else:
                assert shape[w][3] == cout == cin, "plain depthwise only"
                groups, array = cin, constant(w).transpose(3, 0, 1, 2)
            op = {"op": "conv", "in": tensor(at), "kernel": kernel, "stride": stride, "pad": padded, "groups": groups,
                  "cin": cin, "cout": cout, "w": weights.push(array), "b": weights.push(constant(b))}
            after = users.get(out, [])
            if len(after) == 1 and after[0]["op_name"] == "ADD":
                other = [int(i) for i in after[0]["inputs"] if int(i) != out][0]
                skip = other
                if producer[other]["op_name"] == "PAD":
                    assert users[other] == [after[0]]
                    skip = int(producer[other]["inputs"][0])
                op["skip"], op["skip_c"] = tensor(skip), shape[skip][3]
                out = int(after[0]["outputs"][0])
                after = users.get(out, [])
            if len(after) == 1 and after[0]["op_name"] == "PRELU":
                op["act"], op["alpha"] = 2, weights.push(constant(int(after[0]["inputs"][1])))
                out = int(after[0]["outputs"][0])
            op["out"] = tensor(out)
            plan.append(op)
        elif name == "MAX_POOL_2D":
            plan.append({"op": "pool", "in": tensor(int(o["inputs"][0])), "out": tensor(int(o["outputs"][0]))})
        elif name not in ("DEQUANTIZE", "ADD", "PRELU", "PAD", "LOGISTIC", "RESHAPE"):
            raise SystemExit(f"unexpected {name}")

    outputs = {}
    for d in it.get_output_details():
        if d["shape"].tolist() == [1, 1, 1, 1434]:
            outputs["points"] = str(d["index"])
        elif d["shape"].tolist() == [1, 1, 1, 1]:
            outputs["present"] = str(d["index"])
    return {"format": "f16", "ops": plan, "outputs": outputs}, weights.all()


def head(path):
    import onnx
    from onnx import numpy_helper

    graph = onnx.load(path).graph
    initial = {t.name: numpy_helper.to_array(t) for t in graph.initializer}
    users = {}
    for n in graph.node:
        for i in n.input:
            users.setdefault(i, []).append(n)
    attrs = lambda n: {a.name: onnx.helper.get_attribute_value(a) for a in n.attribute}
    weights, plan = Weights(), []
    gemm_out = None
    for n in graph.node:
        if n.op_type == "Conv":
            a = attrs(n)
            w, b = initial[n.input[1]], initial[n.input[2]]
            op = {"op": "conv", "in": n.input[0], "kernel": int(a["kernel_shape"][0]), "stride": int(a["strides"][0]),
                  "pad": int(a["pads"][0]), "groups": int(a["group"]), "cin": int(w.shape[1]) * int(a["group"]),
                  "cout": int(w.shape[0]), "w": weights.push(w), "b": weights.push(b)}
            out, after = n.output[0], users.get(n.output[0], [])
            if len(after) == 1 and after[0].op_type == "Add":
                other = [i for i in after[0].input if i != out][0]
                op["skip"], op["skip_c"] = other, int(w.shape[0])
                out, after = after[0].output[0], users.get(after[0].output[0], [])
            if len(after) == 1 and after[0].op_type == "Clip":
                op["act"] = 3
                out = after[0].output[0]
            op["out"] = out
            plan.append(op)
        elif n.op_type == "GlobalAveragePool":
            plan.append({"op": "gap", "in": n.input[0], "out": n.output[0]})
        elif n.op_type == "Gemm":
            # A fully connected layer is a 1 x 1 convolution of a 1 x 1 picture.
            w, b = initial[n.input[1]], initial[n.input[2]]
            flattened = n.input[0]
            source = next(m for m in graph.node if flattened in m.output)
            assert source.op_type == "Flatten"
            gemm_out = n.output[0]
            plan.append({"op": "conv", "in": source.input[0], "out": gemm_out, "kernel": 1, "stride": 1, "pad": 0, "groups": 1,
                         "cin": int(w.shape[1]), "cout": int(w.shape[0]), "w": weights.push(w[:, :, None, None]), "b": weights.push(b)})
    return {"format": "f16", "ops": plan, "outputs": {"six": gemm_out}}, weights.all()


def square(picture, size):
    from PIL import Image

    image = Image.open(picture).convert("RGB")
    side = min(image.size)
    left, top = (image.width - side) // 2, (image.height - side) // 2
    return np.asarray(image.crop((left, top, left + side, top + side)).resize((size, size), Image.BILINEAR))


def reference(kind, path, picture, rgb_out, out):
    if kind == "mesh":
        rgb = square(picture, MESH_SIZE)
        it = mesh_interpreter(path)
        it.set_tensor(it.get_input_details()[0]["index"], (rgb.astype(np.float32) / 255.0)[None])
        it.invoke()
        details = {tuple(d["shape"].tolist()): d["index"] for d in it.get_output_details()}
        result = np.concatenate([it.get_tensor(details[(1, 1, 1, 1434)]).ravel(), it.get_tensor(details[(1, 1, 1, 1)]).ravel()])
    else:
        import onnxruntime as ort

        rgb = square(picture, HEAD_SIZE)
        x = ((rgb.astype(np.float32) / 255.0 - MEAN) / STD).transpose(2, 0, 1)[None].astype(np.float32)
        session = ort.InferenceSession(path, providers=["CPUExecutionProvider"])
        result = session.run(None, {session.get_inputs()[0].name: x})[0].ravel()
    rgb.tofile(rgb_out)
    result.astype("<f4").tofile(out)
    print("wrote", out, result.shape)


def main():
    kind, path = sys.argv[1:3]
    if "--reference" in sys.argv:
        reference(kind, path, *sys.argv[sys.argv.index("--reference") + 1:])
        return
    plan, weights = (mesh if kind == "mesh" else head)(path)
    text = json.dumps(plan, separators=(",", ":")).encode()
    with open(sys.argv[3], "wb") as f:
        f.write(struct.pack("<I", len(text)) + text + weights.astype("<f2").tobytes())
    print(len(plan["ops"]), "ops,", len(weights), "weights")


main()
