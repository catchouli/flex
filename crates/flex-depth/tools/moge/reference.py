"""Reference outputs for flex-depth's MoGe-2 tests: runs MoGe-2 ViT-S (github.com/microsoft/MoGe,
weights Ruicheng/moge-2-vits-normal) on an image and saves the input and every stage's output, so
the compute shaders can be checked against PyTorch stage by stage.

    PYTHONPATH=<MoGe checkout> python reference.py <model.pt> <image> <out.safetensors> [cols rows]

Needs torch, safetensors, pillow and numpy; MoGe's other requirements are only for things this
doesn't use, and are stood in for. The picture is cols x rows patches of 14 pixels (default 25 x
14), resized to that and saved as RGB bytes with the rest.
"""

import importlib.abc
import importlib.machinery
import sys
import types

import numpy as np
import torch
from PIL import Image
from safetensors.torch import save_file


class Absent(importlib.abc.MetaPathFinder, importlib.abc.Loader):
    """Empty modules for MoGe's imports that inference at a fixed size never touches."""

    roots = ("scipy", "cv2", "utils3d", "utils3d_moge", "pipeline", "flex_gemm", "trimesh", "gradio")

    def find_spec(self, name, path, target=None):
        if name.split(".")[0] in self.roots:
            return importlib.machinery.ModuleSpec(name, self, is_package=True)

    def create_module(self, spec):
        m = types.ModuleType(spec.name)
        m.__path__ = []
        m.__getattr__ = lambda attr: None
        return m

    def exec_module(self, module):
        pass


sys.meta_path.insert(0, Absent())
from moge.model.v2 import MoGeModel  # noqa: E402


def main():
    weights, image, out = sys.argv[1], sys.argv[2], sys.argv[3]
    cols, rows = (int(sys.argv[4]), int(sys.argv[5])) if len(sys.argv) > 5 else (25, 14)
    model = MoGeModel.from_pretrained(weights).eval()
    rgb = np.asarray(Image.open(image).convert("RGB").resize((cols * 14, rows * 14), Image.BICUBIC))
    x = torch.from_numpy(rgb.astype(np.float32) / 255.0).permute(2, 0, 1)[None]

    saved = {"rgb": torch.from_numpy(rgb.copy())}
    backbone = model.encoder.backbone
    for i in model.encoder.intermediate_layers:
        backbone.blocks[i].register_forward_hook(lambda m, inp, o, i=i: saved.__setitem__(f"tokens.{i}", backbone.norm(o)[0].detach().contiguous()))
    model.encoder.register_forward_hook(lambda m, inp, o: saved.__setitem__("features", o[0][0].detach().contiguous()))
    model.neck.register_forward_hook(lambda m, inp, o: saved.update({f"neck.{k}": f[0].detach().contiguous() for k, f in enumerate(o)}))
    for head in ("points", "normal"):
        getattr(model, f"{head}_head").register_forward_hook(lambda m, inp, o, head=head: saved.__setitem__(head, o[-1][0].detach().contiguous()))
    with torch.no_grad():
        model.forward(x, num_tokens=rows * cols)

    # flex-depth works the last level out at half size: its doubling (a bilinear upsample and a
    # 3x3 convolution) becomes just the convolution, with the view plane's u, v at that size.
    import torch.nn as nn
    import moge.model.v2 as v2

    stacks = [model.neck, model.points_head, model.normal_head, model.mask_head]
    for stack in stacks:
        stack.resamplers[3] = nn.Sequential(stack.resamplers[3][1])
    full_uv = v2.normalized_view_plane_uv

    def half_uv(width, height, aspect_ratio=None, dtype=None, device=None):
        if width >= cols * 16:
            width, height = width // 2, height // 2
        return full_uv(width=width, height=height, aspect_ratio=aspect_ratio, dtype=dtype, device=device)

    v2.normalized_view_plane_uv = half_uv
    full = {k: saved[k] for k in ("normal", "points")}
    with torch.no_grad():
        model.forward(x, num_tokens=rows * cols)
    for k in ("normal", "points"):
        saved[f"{k}.half"] = saved[k]
        saved[k] = full[k]
    for k in sorted(saved):
        print(k, tuple(saved[k].shape))
    save_file(saved, out)


if __name__ == "__main__":
    main()
