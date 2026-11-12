"""Runs run.py (the MoGe-2 scene and body-part head's training) on Modal instead of Kaggle.

    modal run --detach modal_run.py                    # start (5.5 hours of training)
    modal app logs flex-moge-segment                   # follow it
    modal volume get flex-moge-segment-out / runs/moge-segment   # what it's made so far

run.py fetches everything itself (SEG_FETCH=1): COCO and ADE20K from where they're published,
DensePose-COCO's labels from Meta, ATR, skytnt's anime-segmentation and MoGe-2 from Hugging Face,
CIHP and Danbooru-Parsing from their authors' Google Drive, MoGe's code from GitHub, onto the
machine's own disk. What it writes to its output (head.safetensors, the best so far;
metrics.json; samples_*.jpg; resume.pt) goes to the volume flex-moge-segment-out, saved every
2 minutes while it runs. If Modal takes the machine back, it restarts the run, which carries on
from resume.pt (written at each checkpoint, every half hour) instead of starting again.
"""

import os
import subprocess
import threading
import time

import modal

MINUTES = int(os.environ.get("SEG_MINUTES", 330))
GPU = os.environ.get("SEG_GPU", "L4")

app = modal.App("flex-moge-segment")
out = modal.Volume.from_name("flex-moge-segment-out", create_if_missing=True)
image = (
    modal.Image.debian_slim(python_version="3.12")
    # ca-certificates: curl needs them for https, and slim images leave them out.
    .apt_install("git", "curl", "unzip", "ca-certificates")
    .pip_install("torch==2.8.0", "numpy", "pillow", "safetensors", "huggingface_hub", "pyarrow")
    .add_local_file(os.path.join(os.path.dirname(__file__), "run.py"), "/root/run.py")
)


@app.function(image=image, gpu=GPU, cpu=8, memory=32768, volumes={"/out": out}, timeout=(MINUTES + 60) * 60)
def train(run: str):
    env = dict(
        os.environ,
        # A restart after preemption gets the same input, so carries on from the last checkpoint.
        SEG_RUN=run,
        SEG_FETCH="1",
        SEG_WORKERS="8",
        SEG_CIHP="/tmp/cihp",
        SEG_ANIME_FACES="/tmp/danbooru-parsing",
        SEG_ANIME_CUTOUTS="/tmp/anime-cutouts",
        SEG_ROOT="/tmp/data",
        SEG_OUT="/out",
        SEG_MOGE="/tmp/MoGe",
        SEG_PANOPTIC="/tmp/panoptic",
        SEG_DENSEPOSE="/tmp/densepose",
        SEG_MINUTES=str(MINUTES),
        PYTHONUNBUFFERED="1",
    )
    done = threading.Event()

    def save():
        # Often, so a preempted run loses little: the checkpoint is saved every half hour.
        while not done.wait(120):
            out.commit()

    threading.Thread(target=save, daemon=True).start()
    try:
        subprocess.run(["python", "/root/run.py"], env=env, check=True)
    finally:
        done.set()
        out.commit()


@app.local_entrypoint()
def main():
    train.remote(time.strftime("%Y%m%d-%H%M%S"))
