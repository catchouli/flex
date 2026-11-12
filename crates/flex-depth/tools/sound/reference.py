#!/usr/bin/env python3
"""A reference for src/sound.rs's tests: YAMNet as in github.com/tensorflow/models
(research/audioset/yamnet, features.py and yamnet.py), in numpy from the original Keras weights,
on the first 0.975 s of a sound. Saves the samples, the log mel spectrogram, every layer's output
and the scores.

    reference.py yamnet.h5 <16 kHz mono 16-bit wav> <out.safetensors>

Needs h5py, numpy and safetensors. The wav can be made with
`ffmpeg -i <film> -ss <time> -t 2 -ac 1 -ar 16000 out.wav`.
"""

import sys
import wave

import h5py
import numpy as np
from safetensors.numpy import save_file

RATE = 16000
WINDOW = 400
HOP = 160
FFT = 512
SAMPLES = 15600  # 0.96 s of frames plus the rest of the last window
MEL_BANDS = 64
EPSILON = 1e-4
LAYERS = [(2, 32), (1, 64), (2, 128), (1, 128), (2, 256), (1, 256), (2, 512), (1, 512), (1, 512), (1, 512), (1, 512), (1, 512), (2, 1024), (1, 1024)]


def mel(hz):
    return 1127.0 * np.log1p(hz / 700.0)


def mel_matrix():
    """tf.signal.linear_to_mel_weight_matrix(64, 257, 16000, 125, 7500)."""
    bins = FFT // 2 + 1
    linear = mel(np.linspace(0.0, RATE / 2, bins, dtype=np.float32)[1:])[:, None]
    edges = np.linspace(mel(125.0), mel(7500.0), MEL_BANDS + 2, dtype=np.float32)
    lower, center, upper = edges[:-2], edges[1:-1], edges[2:]
    w = np.maximum(0.0, np.minimum((linear - lower) / (center - lower), (upper - linear) / (upper - center)))
    return np.concatenate([np.zeros((1, MEL_BANDS), np.float32), w.astype(np.float32)])


def log_mel(samples):
    frames = 1 + (len(samples) - WINDOW) // HOP
    hann = 0.5 - 0.5 * np.cos(2 * np.pi * np.arange(WINDOW) / WINDOW)
    framed = np.stack([samples[i * HOP:i * HOP + WINDOW] * hann for i in range(frames)])
    magnitude = np.abs(np.fft.rfft(framed, FFT))
    return np.log(magnitude @ mel_matrix() + 0.001).astype(np.float32)


def pad(x, stride):
    """TensorFlow's "same" padding for a 3x3 window: 1 each side, or 0 before and 1 after for
    stride 2 on even sizes."""
    return np.pad(x, ((1, 1), (1, 1), (0, 0))) if stride == 1 else np.pad(x, ((0, 1), (0, 1), (0, 0)))


def batch_norm(f, name, x):
    bn = f[f"{name}/bn/{name}/bn"]
    return np.maximum((x - bn["moving_mean:0"][()]) / np.sqrt(bn["moving_variance:0"][()] + EPSILON) + bn["beta:0"][()], 0)


def conv(x, k, stride, depthwise):
    """x [H, W, C], k [3, 3, in, out] (or [3, 3, C, 1] depthwise)."""
    p = pad(x, stride)
    h, w = (x.shape[0] + stride - 1) // stride, (x.shape[1] + stride - 1) // stride
    out = 0
    for dy in range(3):
        for dx in range(3):
            patch = p[dy:dy + stride * h:stride, dx:dx + stride * w:stride]
            out = out + (patch * k[dy, dx, :, 0] if depthwise else patch @ k[dy, dx])
    return out


def main():
    h5, wav, out = sys.argv[1:4]
    with wave.open(wav) as w:
        assert w.getframerate() == RATE and w.getnchannels() == 1 and w.getsampwidth() == 2
        samples = np.frombuffer(w.readframes(SAMPLES), np.int16).astype(np.float32) / 32768.0
    assert len(samples) == SAMPLES
    f = h5py.File(h5, "r")
    spectrogram = log_mel(samples)
    t = {"samples": samples, "log_mel": spectrogram}
    x = spectrogram[:, :, None].astype(np.float64)
    for n, (stride, _) in enumerate(LAYERS, 1):
        p = f"layer{n}"
        if n == 1:
            x = batch_norm(f, f"{p}/conv", conv(x, f[f"{p}/conv/{p}/conv/kernel:0"][()], stride, False))
        else:
            x = batch_norm(f, f"{p}/depthwise_conv", conv(x, f[f"{p}/depthwise_conv/{p}/depthwise_conv/depthwise_kernel:0"][()], stride, True))
            x = batch_norm(f, f"{p}/pointwise_conv", x @ f[f"{p}/pointwise_conv/{p}/pointwise_conv/kernel:0"][()][0, 0])
        t[f"l{n}"] = x.astype(np.float32)
    logits = x.mean(axis=(0, 1)) @ f["logits/logits/kernel:0"][()] + f["logits/logits/bias:0"][()]
    t["scores"] = (1 / (1 + np.exp(-logits))).astype(np.float32)
    save_file(t, out)
    top = np.argsort(-t["scores"])[:5]
    print("top classes:", ", ".join(f"{i} ({t['scores'][i]:.3f})" for i in top))


if __name__ == "__main__":
    main()
