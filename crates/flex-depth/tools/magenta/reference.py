"""Reference outputs for flex-depth's Magenta style transfer tests: runs Magenta's arbitrary image
stylization (Kaggle Models' google/arbitrary-image-stylization-v1, TensorFlow 1, 256, version 2;
the SavedModel unpacked) with TensorFlow on a picture and a style picture, and saves the inputs
with the style network's and the painting network's stages, so the ports can be checked stage
by stage.

    python reference.py <saved_model_dir> <picture> <style picture> <out.safetensors> [width height]

Needs tensorflow, safetensors, pillow and numpy. The picture is resized to width x height
(default 256 x 160); the style picture is cropped to a square in the middle, resized to 256 x 256
and smoothed with a 3x3 average, as the model's own example does. Stages are channels first,
[channels][height][width], as flex-depth keeps them; the style vector is [768].
"""

import sys

import numpy as np
import tensorflow as tf
from PIL import Image
from safetensors.numpy import save_file
from tensorflow.python.framework import function_def_to_graph, tensor_util


def flatten(path):
    """The model's one function as a plain GraphDef, its variables as constants."""
    m = tf.saved_model.load(path)
    f = m.signatures["serving_default"]
    inner = [o for o in f.graph.get_operations() if o.type == "StatefulPartitionedCall"][0]
    called = inner.get_attr("f").name
    fdef = next(fd for fd in f.graph.as_graph_def().library.function if fd.signature.name == called)
    by_handle = {id(v.handle): v for v in m.variables}
    captured = {inn.name: by_handle[id(ext)] for ext, inn in f.graph.captures if id(ext) in by_handle}
    values = {}
    for i, a in enumerate(fdef.signature.input_arg):
        t = inner.inputs[i]
        if t.name in captured:
            values[a.name] = captured[t.name].numpy()
    gd = function_def_to_graph.function_def_to_graph(fdef).as_graph_def()
    out = tf.compat.v1.GraphDef()
    for n in gd.node:
        if n.op == "Placeholder" and n.name in values:
            continue
        if n.op == "ReadVariableOp" and n.input[0] in values:
            v = values[n.input[0]]
            c = out.node.add()
            c.name, c.op = n.name, "Const"
            c.attr["dtype"].type = tf.as_dtype(v.dtype).as_datatype_enum
            c.attr["value"].tensor.CopyFrom(tensor_util.make_tensor_proto(v))
            continue
        out.node.add().CopyFrom(n)
    return out


def style_picture(path):
    im = Image.open(path).convert("RGB")
    s = min(im.size)
    l, t = (im.width - s) // 2, (im.height - s) // 2
    im = im.crop((l, t, l + s, t + s)).resize((256, 256), Image.BILINEAR)
    x = tf.constant(np.asarray(im, np.float32)[None] / 255.0)
    return tf.nn.avg_pool(x, ksize=[3, 3], strides=[1, 1], padding="SAME").numpy()


STAGES = {
    "style.conv": "InceptionV3/MaxPool_5a_3x3/MaxPool",
    "style.5b": "InceptionV3/Mixed_5b/concat",
    "style.5d": "InceptionV3/Mixed_5d/concat",
    "style.6a": "InceptionV3/Mixed_6a/concat",
    "style.6b": "InceptionV3/Mixed_6b/concat",
    "style.6e": "InceptionV3/Mixed_6e/concat",
    "style": "bottleneck/Mean",
    "down.0": "transformer/contract/conv1/Relu",
    "down.1": "transformer/contract/conv2/Relu",
    "down.2": "transformer/contract/conv3/Relu",
    "res.0": "transformer/residual/residual1/add",
    "res.9": "transformer/residual/residual10/add",
    "res.19": "transformer/residual/residual20/add",
    "up.0": "transformer/expand/conv1/conv/Relu",
    "up.1": "transformer/expand/conv2/conv/Relu",
    "out": "transformer/expand/conv3/conv/Sigmoid",
}


def main():
    model, picture, style, out = sys.argv[1:5]
    w, h = (int(sys.argv[5]), int(sys.argv[6])) if len(sys.argv) > 6 else (256, 160)
    content = np.asarray(Image.open(picture).convert("RGB").resize((w, h), Image.BILINEAR), np.uint8)
    s = style_picture(style)
    flat = flatten(model)
    g = tf.Graph()
    with g.as_default():
        tf.compat.v1.import_graph_def(flat, name="")
    with tf.compat.v1.Session(graph=g) as sess:
        names = {k: v + ":0" for k, v in STAGES.items()}
        got = sess.run(names, {"placeholder:0": content[None].astype(np.float32) / 255.0, "placeholder_1:0": s})
    tensors = {"rgb": content, "style_picture": s[0].transpose(2, 0, 1).copy()}
    for k, v in got.items():
        tensors[k] = v[0].transpose(2, 0, 1).copy() if v.ndim == 4 and k != "style" else v.reshape(-1).copy()
        print(k, tensors[k].shape)
    save_file({k: np.ascontiguousarray(v) for k, v in tensors.items()}, out)


if __name__ == "__main__":
    main()
