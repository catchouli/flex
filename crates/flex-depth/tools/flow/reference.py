"""Reference outputs for flex-depth's flow tests: runs NeuFlow v2 as Flex does (no upscaling, two steps)
on two pictures and saves every stage, so the compute shaders can be checked against PyTorch.

    python reference.py <NeuFlow_v2 checkout> <current image> <last image> <out.safetensors>

Needs torch, safetensors, huggingface_hub, pillow and numpy.
"""

import sys

import numpy as np
import torch
import torch.nn.functional as F
from PIL import Image
from safetensors.torch import save_file

from convert import load

WIDTH, HEIGHT, STEPS = 512, 288, 2


def picture(path):
    return np.asarray(Image.open(path).convert("RGB").resize((WIDTH, HEIGHT), Image.BICUBIC))


def main():
    checkout, current, last, out = sys.argv[1:5]
    model = load(checkout)
    model.init_bhwd(1, HEIGHT, WIDTH, "cpu", amp=False)
    rgb0, rgb1 = picture(current), picture(last)
    saved = {"rgb0": torch.from_numpy(rgb0.copy()), "rgb1": torch.from_numpy(rgb1.copy())}
    # Its weights expect BGR, 0 to 255.
    a, b = [torch.from_numpy(x[..., ::-1].copy()).permute(2, 0, 1).float()[None] / 255.0 for x in (rgb0, rgb1)]
    with torch.no_grad():
        s16, s8 = model.backbone(torch.cat([a, b]))
        saved["backbone_16"], saved["backbone_8"] = s16[0], s8[0]
        attn = model.cross_attn_s16(s16)
        saved["attention"] = attn[0]
        feats16, context16 = model.split_features(attn, 64, 128)
        feats8, context8 = model.split_features(s8, 64, 128)
        f0, f1 = feats16.chunk(2)
        flow = model.matching_s16.global_correlation_softmax(f0, f1)
        saved["matching"] = flow[0]
        corr = model.corr_block_s16.init_corr_pyr(f0, f1)
        hidden = model.init_iter_context_s16
        corrs = model.corr_block_s16(corr, flow)
        saved["corr_16"] = corrs[0]
        hidden, delta = model.refine_s16(corrs, context16, hidden, flow)
        flow = flow + delta
        saved["flow_16"] = flow[0]
        flow = F.interpolate(flow, scale_factor=2, mode="nearest") * 2
        up = F.interpolate(feats16, scale_factor=2, mode="nearest")
        merged = model.merge_s8(torch.cat([feats8, up], dim=1))
        saved["merged"] = merged[0]
        context16 = F.interpolate(context16, scale_factor=2, mode="nearest")
        context = model.context_merge_s8(torch.cat([context8, context16], dim=1))
        saved["context_8"] = context[0]
        f0, f1 = merged.chunk(2)
        corr = model.corr_block_s8.init_corr_pyr(f0, f1)
        hidden = model.init_iter_context_s8
        for step in range(STEPS):
            hidden, delta = model.refine_s8(model.corr_block_s8(corr, flow), context, hidden, flow)
            flow = flow + delta
            saved[f"step_{step}"] = flow[0]
    save_file({k: v.detach().contiguous().float() if v.dtype != torch.uint8 else v.contiguous() for k, v in saved.items()}, out)


if __name__ == "__main__":
    main()
