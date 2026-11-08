"""Reference outputs for flex-depth's tests: runs Depth Anything V2 Small (transformers)
on an image and saves the input and every stage's output, so the compute shaders can be
checked against PyTorch stage by stage.

    python reference.py <image> <out.safetensors> [width height]

Needs torch, transformers, safetensors, pillow and numpy. The image is resized to
width x height (multiples of 14; default 518 x 294) and saved as RGB bytes with the rest.
"""

import sys

import numpy as np
import torch
from PIL import Image
from safetensors.torch import save_file
from transformers import AutoModelForDepthEstimation

MODEL = "depth-anything/Depth-Anything-V2-Small-hf"
REVISION = "5426e4f0f36572d16453bbda7a8389317b1bef99"
MEAN = [0.485, 0.456, 0.406]
STD = [0.229, 0.224, 0.225]


def main():
    image, out = sys.argv[1], sys.argv[2]
    width, height = (int(sys.argv[3]), int(sys.argv[4])) if len(sys.argv) > 4 else (518, 294)
    assert width % 14 == 0 and height % 14 == 0
    model = AutoModelForDepthEstimation.from_pretrained(MODEL, revision=REVISION).eval()
    print(model.config.backbone_config.out_features, model.config.depth_estimation_type, model.config.max_depth)

    rgb = np.asarray(Image.open(image).convert("RGB").resize((width, height), Image.BICUBIC))
    x = torch.from_numpy(rgb.astype(np.float32) / 255.0).permute(2, 0, 1)
    x = ((x - torch.tensor(MEAN)[:, None, None]) / torch.tensor(STD)[:, None, None])[None]

    saved = {"rgb": torch.from_numpy(rgb.copy())}
    hooks = []

    def keep(name):
        def hook(_module, _inputs, output):
            if isinstance(output, (list, tuple)):
                for i, o in enumerate(output):
                    saved[f"{name}.{i}"] = o.detach()[0].contiguous()
            else:
                saved[name] = output.detach()[0].contiguous()

        return hook

    hooks.append(model.backbone.embeddings.register_forward_hook(keep("embeddings")))
    for i, layer in enumerate(model.backbone.encoder.layer):
        hooks.append(layer.register_forward_hook(keep(f"block.{i}")))
    for i, layer in enumerate(model.neck.reassemble_stage.layers):
        hooks.append(layer.register_forward_hook(keep(f"reassemble.{i}")))
    for i, conv in enumerate(model.neck.convs):
        hooks.append(conv.register_forward_hook(keep(f"neck_conv.{i}")))
    hooks.append(model.neck.fusion_stage.register_forward_hook(keep("fusion")))

    with torch.no_grad():
        result = model(pixel_values=x, output_hidden_states=True)
        features = model.backbone(pixel_values=x).feature_maps
    for i, f in enumerate(features):
        saved[f"features.{i}"] = f[0].contiguous()
    saved["depth"] = result.predicted_depth[0].contiguous()
    # Block hooks may return tuples; keep only tensors.
    saved = {k: v for k, v in saved.items() if isinstance(v, torch.Tensor)}
    for k in sorted(saved):
        print(k, tuple(saved[k].shape))
    save_file(saved, out)


if __name__ == "__main__":
    main()
