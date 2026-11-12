"""Turns NeuFlow v2's checkpoint (github.com/neufieldrobotics/NeuFlow_v2, Apache 2.0, the "mixed" weights)
into models/flow.safetensors, the half-precision file src/flow.rs runs. Batch norms are folded into the convolutions before
them, and the upscaling layers, which Flex doesn't use, are left out.

    python convert.py <NeuFlow_v2 checkout> <out.safetensors>

Needs torch, safetensors and huggingface_hub (for importing the checkout).
"""

import sys

import torch
from safetensors.torch import save_file


def fold(conv, norm):
    """A convolution and the batch norm after it, as one convolution."""
    fused = torch.nn.Conv2d(conv.in_channels, conv.out_channels, conv.kernel_size, conv.stride, conv.padding, bias=True).requires_grad_(False)
    with torch.no_grad():
        scale = norm.weight / torch.sqrt(norm.eps + norm.running_var)
        fused.weight.copy_(conv.weight * scale.view(-1, *[1] * (conv.weight.dim() - 1)))
        start = conv.bias if conv.bias is not None else torch.zeros(conv.out_channels)
        fused.bias.copy_(start * scale + norm.bias - norm.weight * norm.running_mean / torch.sqrt(norm.eps + norm.running_var))
    return fused


def rounded(module):
    """Every weight as it'll be stored."""
    for p in module.parameters():
        p.requires_grad_(False)
        p.data.copy_(p.data.half().float())
    return module


def load(checkout):
    """The network as Flex runs it, in PyTorch, for comparing against."""
    sys.path.insert(0, checkout)
    from NeuFlow.backbone_v7 import ConvBlock
    from NeuFlow.neuflow import NeuFlow

    model = NeuFlow()
    model.load_state_dict(torch.load(f"{checkout}/neuflow_mixed.pth", map_location="cpu", weights_only=True)["model"], strict=True)
    model.eval()
    for block in model.modules():
        if type(block) is ConvBlock:
            block.conv1 = fold(block.conv1, block.norm1)
            block.conv2 = fold(block.conv2, block.norm2)
            del block.norm1, block.norm2
            block.forward = block.forward_fuse
    for sequence in (model.merge_s8, model.context_merge_s8):
        sequence[2] = fold(sequence[2], sequence[3])
        sequence[3] = torch.nn.Identity()
    del model.conv_s8, model.upsample_s8
    return rounded(model)


def main():
    checkout, out = sys.argv[1], sys.argv[2]
    model = load(checkout)
    norm = model.cross_attn_s16.norm
    variance = torch.sqrt(norm.eps + norm.running_var)
    tensors = {k: v for k, v in model.state_dict().items() if not k.startswith("cross_attn_s16.norm.")}
    tensors["cross_attn_s16.norm.scale"] = norm.weight / variance
    tensors["cross_attn_s16.norm.shift"] = norm.bias - norm.weight * norm.running_mean / variance
    save_file({k: v.detach().contiguous().half() for k, v in tensors.items()}, out)
    print(len(tensors), "tensors", sum(v.numel() for v in tensors.values()) / 1e6, "M values")


if __name__ == "__main__":
    main()
