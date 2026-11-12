"""Trains the scene and body-part head for flex-depth's MoGe-2 as a Kaggle script kernel (or,
with SEG_FETCH=1, anywhere: it was run on Colab).

    kaggle kernels push -p crates/flex-depth/tools/moge/segment     # start
    kaggle kernels status bombpersons/flex-moge-segment-train
    kaggle kernels output bombpersons/flex-moge-segment-train -p runs/moge-segment

A head on MoGe-2 ViT-S, which stays frozen: it reads the two transformer layers' tokens the
neck reads (normed) and the neck's 64-channel features at 8x the patch grid, which the depth
pass has already worked out. It's built only from what depth.wgsl can run (1x1 and 3x3
convolutions, ReLU, align_corners bilinear resizes, additions).

It gives four sets of answers. What each pixel is, of SCENE: from COCO panoptic and ADE20K's
150-class Scene Parsing set, with their classes mapped by name. For a person's pixels, what
they're made of, of PARTS (skin, hair, clothes): from ATR's human parsing labels (one person
each, full length, in fashion photos), where only the person counts for SCENE. For a person's
pixels, which part of the body they are, of BODY, whatever they're wearing: from DensePose-COCO
(people in COCO's pictures with their body parts drawn), and what ATR can tell (shoes are feet,
faces and hair are heads). And which person each person's pixel belongs to, as CENTRES: how
near each person's centre each pixel is (1 at the centre, a Gaussian), and for a person's pixels
how far it is to their centre (in picture heights, x then y), from COCO's outlines of each
person and from ATR's one person a picture.

For films' close-ups and crowds, and for drawn people: CIHP (crowds of people, each labelled
apart, with their parts); Danbooru-Parsing (anime portraits, with skin, hair and clothes); and
skytnt's anime-segmentation (characters cut out, put over backgrounds without people one to
three at a time at all sizes, and pictures with their characters' masks). Training pictures with
someone big enough in them are closed in on them CLOSE_UPS of the time. Of each batch (MIX): 22%
COCO, 12% COCO pictures with DensePose's people, 27% ADE20K, 10% ATR, 13% CIHP, 5% anime faces,
3% anime pictures and 8% anime cut-outs.
COCO's panoptic labels come from cocodataset.org, DensePose's from Meta's dl.fbaipublicfiles.com,
ATR from Hugging Face (mattmdjaga/human_parsing_dataset), CIHP and Danbooru-Parsing from their
authors' Google Drive, the anime cut-outs from Hugging Face (skytnt/anime-segmentation), the
depth model from Hugging Face at the revision Flex uses.
Everything in /kaggle/working/out comes back as output: head.safetensors (the best so far),
metrics.json, and samples_*.jpg (validation pictures with their masks).

Scored separately on each dataset's validation set, as IoU (ATR's: its last 500 pictures;
DensePose's: its minival set; CIHP's: a quarter of its; Danbooru-Parsing's test set; the last
300 of skytnt's pictures, for PERSON alone), and for CENTRES (COCO's and CIHP's), how many
people's centres are found (F1, each found centre matched to one person) and how many person
pixels point within 6 pixels of theirs.

SEG_ROOT, SEG_OUT, SEG_PANOPTIC, SEG_DENSEPOSE, SEG_CIHP, SEG_ANIME_FACES, SEG_ANIME_CUTOUTS and
SEG_MOGE (where things go), SEG_DENSEPOSE_TRAIN and SEG_DENSEPOSE_VAL (which of DensePose's
files), SEG_ATR_REVISION, SEG_RUN (carry on that run from OUT's resume.pt, if it's there),
SEG_MINUTES, SEG_BATCH, SEG_EVAL_MINUTES, SEG_WORKERS (DataLoader workers) and SEG_LIMIT (only that many pictures of each set, spread through it) are for trying
it out away from Kaggle; with SEG_FETCH=1 it downloads the datasets itself (as on Colab and
Modal).
"""

import glob
import io
import json
import math
import os
import random
import subprocess
import time
import zipfile

import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F
from PIL import Image, ImageEnhance, ImageFilter
from safetensors.torch import save_file
from torch.utils.data import DataLoader, Dataset, Sampler

START = time.time()
MINUTES = float(os.environ.get("SEG_MINUTES", 5 * 60))
EVAL_MINUTES = float(os.environ.get("SEG_EVAL_MINUTES", 30))
BATCH = int(os.environ.get("SEG_BATCH", 32))
LR, WEIGHT_DECAY, WARMUP = 1e-3, 1e-4, 200
ROOT = os.environ.get("SEG_ROOT", "/kaggle/input")
OUT = os.environ.get("SEG_OUT", "/kaggle/working/out")
PANOPTIC = os.environ.get("SEG_PANOPTIC", "/tmp/panoptic")
PANOPTIC_URL = "http://images.cocodataset.org/annotations/panoptic_annotations_trainval2017.zip"

MODEL = "Ruicheng/moge-2-vits-normal"
REVISION = "26b477f41595707c5db6770294c0d1721e8ed4ed"
MOGE_CODE = "https://github.com/microsoft/MoGe.git"
MOGE_COMMIT = "74fbce0"
MOGE_DIR = os.environ.get("SEG_MOGE", "/tmp/MoGe")
# The grid crates/flex-app/src/depth.rs runs the model at.
PATCH, GW, GH = 14, 25, 14
WIDTH, HEIGHT = GW * PATCH, GH * PATCH

SCENE = ["other", "person", "wall", "sky", "ground", "ceiling", "water", "plant", "window", "vehicle", "animal", "building", "furniture"]
OTHER, PERSON, WALL, SKY, GROUND, CEILING, WATER, PLANT, WINDOW, VEHICLE, ANIMAL, BUILDING, FURNITURE = range(len(SCENE))
PARTS = ["skin", "hair", "clothes"]
SKIN, HAIR, CLOTHES = range(len(PARTS))
BODY = ["head", "torso", "arm", "hand", "leg", "foot"]
HEAD, TORSO, ARM, HAND, LEG, FOOT = range(len(BODY))
CENTRES = ["centre", "to centre x", "to centre y"]
CLASSES = SCENE + PARTS + BODY
OUTPUTS = len(CLASSES) + len(CENTRES)
IGNORE = 255
SCENE_WEIGHTS = torch.tensor([0.5] + [1.0] * (len(SCENE) - 1))
COLOURS = np.array([
    [0, 0, 0], [255, 60, 60], [60, 110, 255], [60, 230, 230], [150, 110, 60], [200, 200, 120], [40, 90, 255], [60, 200, 60], [255, 220, 80],
    [255, 140, 0], [200, 80, 255], [140, 140, 160], [120, 70, 40],
], dtype=np.float32)
PART_COLOURS = np.array([[255, 190, 150], [120, 60, 20], [180, 60, 220]], dtype=np.float32)
BODY_COLOURS = np.array([[255, 230, 60], [60, 120, 255], [60, 220, 90], [255, 60, 200], [255, 120, 40], [120, 255, 255]], dtype=np.float32)
# CENTRES: the Gaussian's spread (pixels of the labels, which are WIDTH x HEIGHT), and how much
# its and the offsets' errors count against the classes'.
CENTRE_SIGMA = 6.0
CENTRE_WEIGHT, OFFSET_WEIGHT = 100.0, 5.0
# How often a training picture with someone big enough in it is closed in on them (see crop_box).
CLOSE_UPS = 0.4
# Of each batch: COCO, COCO with DensePose's people, ADE20K, ATR, CIHP, anime faces, anime pictures
# and anime cut-outs.
MIX = [0.22, 0.12, 0.27, 0.1, 0.13, 0.05, 0.03, 0.08]
DENSEPOSE = os.environ.get("SEG_DENSEPOSE", "/tmp/densepose")
DENSEPOSE_URL = "https://dl.fbaipublicfiles.com/densepose"
# DensePose's 14 drawn parts (0 is none): torso, right and left hand, left and right foot, upper
# legs, lower legs, upper arms, lower arms, head.
DP_BODY = np.array([IGNORE, TORSO, HAND, HAND, FOOT, FOOT, LEG, LEG, LEG, LEG, ARM, ARM, ARM, ARM, HEAD], dtype=np.uint8)
ATR = "mattmdjaga/human_parsing_dataset"
ATR_REVISION = os.environ.get("SEG_ATR_REVISION", "db120bb5c18c146a8fbd2160f7575a288269fe7d")
# ATR's labels: 0 background, 1 hat, 2 hair, 3 sunglasses, 4 upper clothes, 5 skirt, 6 pants,
# 7 dress, 8 belt, 9 and 10 shoes, 11 face, 12 and 13 legs, 14 and 15 arms, 16 bag, 17 scarf.
ATR_PART = np.full(256, IGNORE, dtype=np.uint8)
ATR_PART[[11, 12, 13, 14, 15]] = SKIN
ATR_PART[2] = HAIR
ATR_PART[[1, 4, 5, 6, 7, 8, 9, 10, 17]] = CLOTHES
ATR_SCENE = np.full(256, PERSON, dtype=np.uint8)
ATR_SCENE[[0, 16]] = IGNORE  # The background could be anything; a bag isn't the person.
# What ATR can tell of BODY. Its arms are only bare skin (sleeves are clothes) and take in the
# hands, and tops, skirts and dresses cover more than one part: none of those count.
ATR_BODY = np.full(256, IGNORE, dtype=np.uint8)
ATR_BODY[[1, 2, 3, 11]] = HEAD
ATR_BODY[8] = TORSO
ATR_BODY[[6, 12, 13]] = LEG
ATR_BODY[[9, 10]] = FOOT
# SCENE's label for "not a person, but what it is isn't known" (backgrounds that hold no people):
# it counts against PERSON only.
NOT_PERSON = 254
# CIHP (crowds, each person labelled apart): 0 background, 1 hat, 2 hair, 3 glove, 4 sunglasses,
# 5 upper clothes, 6 dress, 7 coat, 8 socks, 9 pants, 10 torso skin, 11 scarf, 12 skirt, 13 face,
# 14 and 15 arms, 16 and 17 legs, 18 and 19 shoes. Some small people far off aren't labelled, so
# its background doesn't count for SCENE.
CIHP = os.environ.get("SEG_CIHP", "/tmp/cihp")
CIHP_URL = "https://drive.usercontent.google.com/download?id=1HdqA8yWVxZ8od0hngKzZTziHx_ONzCWj&export=download&confirm=t"
CIHP_PART = np.full(256, IGNORE, dtype=np.uint8)
CIHP_PART[[10, 13, 14, 15, 16, 17]] = SKIN
CIHP_PART[2] = HAIR
CIHP_PART[[1, 3, 5, 6, 7, 8, 9, 11, 12, 18, 19]] = CLOTHES
CIHP_SCENE = np.full(256, PERSON, dtype=np.uint8)
CIHP_SCENE[0] = IGNORE
# As ATR's: bare arms take in the hands, and tops, dresses and coats cover more than one part.
CIHP_BODY = np.full(256, IGNORE, dtype=np.uint8)
CIHP_BODY[[1, 2, 4, 13]] = HEAD
CIHP_BODY[3] = HAND
CIHP_BODY[10] = TORSO
CIHP_BODY[[9, 16, 17]] = LEG
CIHP_BODY[[18, 19]] = FOOT
# Danbooru-Parsing (anime portraits, from StyleAnime): 0 background, 1 skin, 2 and 3 brows, 4 and
# 5 eyes, 6 glasses, 7 and 8 ears, 9 earring, 10 nose, 11 mouth, 12 and 13 lips, 14 neck,
# 15 necklace, 16 clothes, 17 hair, 18 hat. Hands and what they hold are left as background, so
# only background well away from the person counts as NOT_PERSON (see `anime_face_classes`).
ANIME_FACES = os.environ.get("SEG_ANIME_FACES", "/tmp/danbooru-parsing")
ANIME_FACES_URL = "https://drive.usercontent.google.com/download?id=13C7Jphi8dYkY_0HoqIZeXAIkppyKlzmQ&export=download&confirm=t"
AF_PART = np.full(256, IGNORE, dtype=np.uint8)
AF_PART[[1, 2, 3, 4, 5, 7, 8, 10, 11, 12, 13, 14]] = SKIN
AF_PART[17] = HAIR
AF_PART[[16, 18]] = CLOTHES
AF_BODY = np.full(256, IGNORE, dtype=np.uint8)
AF_BODY[[1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 17, 18]] = HEAD
AF_PERSON = np.zeros(256, dtype=bool)
AF_PERSON[1:] = True
# skytnt's anime-segmentation: characters cut out (fg, RGBA) and backgrounds without people (bg),
# put together here, and pictures with their characters' masks (imgs-masks), from Hugging Face.
ANIME_CUTOUTS = "skytnt/anime-segmentation"
ANIME_CUTOUTS_REVISION = "6685505e1e3c02ac0483398e633922b31de89fb0"
ANIME_CUTOUTS_DIR = os.environ.get("SEG_ANIME_CUTOUTS", "/tmp/anime-cutouts")


def scene_of(name):
    """SCENE's class for a COCO or ADE20K class name (ADE20K's are lists of synonyms)."""
    n = name.split(",")[0].strip().lower()
    if n in ("person",):
        return PERSON
    if n.startswith("wall"):
        return WALL
    if n == "sky" or n.startswith("sky-"):
        return SKY
    if n.startswith("floor") or n in ("rug-merged", "rug", "pavement-merged", "sidewalk", "road", "dirt-merged", "earth", "sand", "gravel", "snow",
                                      "playingfield", "field", "railroad", "platform", "grass-merged", "grass", "path", "runway", "dirt track", "land"):
        return GROUND
    if n.startswith("ceiling"):
        return CEILING
    if n in ("water-other", "water", "sea", "river", "lake", "swimming pool", "waterfall"):
        return WATER
    if n in ("tree-merged", "tree", "flower", "potted plant", "plant", "palm"):
        return PLANT
    if n.startswith("window"):
        return WINDOW
    if n in ("bicycle", "car", "motorcycle", "airplane", "bus", "train", "truck", "boat", "van", "ship", "minibike"):
        return VEHICLE
    if n in ("bird", "cat", "dog", "horse", "sheep", "cow", "elephant", "bear", "zebra", "giraffe", "animal"):
        return ANIMAL
    if n in ("house", "roof", "building-other-merged", "building", "skyscraper", "tower"):
        return BUILDING
    if n in ("bench", "chair", "couch", "bed", "dining table", "table-merged", "cabinet-merged", "shelf", "counter", "table", "sofa", "cabinet",
             "armchair", "seat", "desk", "wardrobe", "chest of drawers", "bookcase", "coffee table", "stool", "swivel chair", "kitchen island"):
        return FURNITURE
    return OTHER
SAMPLES = 8

device = "cuda" if torch.cuda.is_available() else "cpu"
# The CPUs this may use (a container's share, not the whole machine's).
WORKERS = int(os.environ.get("SEG_WORKERS", len(os.sched_getaffinity(0))))


def sh(cmd):
    print(f"$ {cmd}", flush=True)
    r = subprocess.run(cmd, shell=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    print(r.stdout[-3000:], flush=True)
    if r.returncode:
        raise RuntimeError(f"failed ({r.returncode}): {cmd}")


def find(tail):
    """A mounted dataset's folder, however deep Kaggle put it (without walking the pictures)."""
    for depth in range(1, 5):
        found = glob.glob(os.path.join(ROOT, *["*"] * depth, tail))
        if found:
            return found[0]
    raise FileNotFoundError(f"{tail} isn't under {ROOT}: {os.listdir(ROOT)}")


COCO_ZIPS = ["http://images.cocodataset.org/zips/train2017.zip", "http://images.cocodataset.org/zips/val2017.zip"]
ADE_ZIP = "https://data.csail.mit.edu/places/ADEchallenge/ADEChallengeData2016.zip"


def fetch_data():
    """Away from Kaggle (SEG_FETCH=1): COCO's pictures and ADE20K from where they're published,
    laid out under ROOT as Kaggle's datasets are."""
    coco = f"{ROOT}/coco/coco2017"
    os.makedirs(coco, exist_ok=True)
    for url in COCO_ZIPS:
        split = url.rsplit("/", 1)[1][:-4]
        if not os.path.exists(f"{coco}/{split}"):
            sh(f"curl -sSfL -o /tmp/{split}.zip {url} && unzip -q /tmp/{split}.zip -d {coco} && rm /tmp/{split}.zip")
    if not os.path.exists(f"{ROOT}/ade/ADEChallengeData2016"):
        os.makedirs(f"{ROOT}/ade", exist_ok=True)
        sh(f"curl -sSfL -o /tmp/ade.zip {ADE_ZIP} && unzip -q /tmp/ade.zip -d {ROOT}/ade && rm /tmp/ade.zip")


def get_panoptic():
    """COCO's panoptic annotations, unpacked: a zip of two JSON files and two more zips."""
    if os.path.exists(f"{PANOPTIC}/annotations/panoptic_val2017"):
        return
    os.makedirs(PANOPTIC, exist_ok=True)
    sh(f"curl -sSfL -o {PANOPTIC}/panoptic.zip {PANOPTIC_URL}")
    with zipfile.ZipFile(f"{PANOPTIC}/panoptic.zip") as z:
        z.extractall(PANOPTIC)
    for split in ("train2017", "val2017"):
        with zipfile.ZipFile(f"{PANOPTIC}/annotations/panoptic_{split}.zip") as z:
            z.extractall(f"{PANOPTIC}/annotations")
        os.remove(f"{PANOPTIC}/annotations/panoptic_{split}.zip")
    os.remove(f"{PANOPTIC}/panoptic.zip")


# ---- Data ------------------------------------------------------------------------------------


def coco_items(split, images):
    """(picture, label image, sorted segment ids, their classes) for each picture."""
    with open(f"{PANOPTIC}/annotations/panoptic_{split}.json") as f:
        data = json.load(f)
    classes = {c["id"]: scene_of(c["name"]) for c in data["categories"]}
    print("COCO:", {SCENE[k]: sorted(c["name"] for c in data["categories"] if classes[c["id"]] == k) for k in range(1, len(SCENE))}, flush=True)
    counts = [sum(v == c for v in classes.values()) for c in (PERSON, WALL, SKY)]
    assert counts[0] == 1 and counts[1] >= 5 and counts[2] == 1, counts
    items = []
    for a in data["annotations"]:
        # Crowds are drawn as one rough blob, so they don't count either way.
        segments = sorted((s["id"], IGNORE if s.get("iscrowd") else classes[s["category_id"]]) for s in a["segments_info"])
        if segments:
            ids, cls = zip(*segments)
            items.append((
                f"{images}/{a['file_name'].replace('.png', '.jpg')}",
                f"{PANOPTIC}/annotations/panoptic_{split}/{a['file_name']}",
                np.array(ids, dtype=np.int64),
                np.array(cls, dtype=np.uint8),
                None,
            ))
    return items


def image_id(item):
    return int(os.path.basename(item[0]).split(".")[0])


# ---- DensePose ------------------------------------------------------------------------------


def densepose(names):
    """DensePose-COCO's people with body parts drawn, from its files `names` (downloaded if need
    be): {COCO picture id: [(box x, y, w, h; its 14 parts' masks, 256 x 256 over the box, as
    COCO's compressed run lengths or None)]}."""
    os.makedirs(DENSEPOSE, exist_ok=True)
    people = {}
    for name in names:
        path = f"{DENSEPOSE}/{name}"
        if not os.path.exists(path):
            # Over plain http too if https fails (Meta's certificate for it expired on 11 November
            # 2026): they're only labels, read as JSON, as COCO's are fetched over http.
            plain = DENSEPOSE_URL.replace("https://", "http://", 1)
            sh(f"(curl -sSfL -o {path}.part {DENSEPOSE_URL}/{name} || curl -sSfL -o {path}.part {plain}/{name}) && mv {path}.part {path}")
        with open(path) as f:
            data = json.load(f)
        for a in data["annotations"]:
            if "dp_masks" in a and not a.get("iscrowd"):
                masks = tuple((m["counts"] if isinstance(m, dict) and m else None) for m in a["dp_masks"])
                people.setdefault(a["image_id"], []).append((tuple(a["bbox"]), masks))
        del data
    return people


def run_lengths(counts):
    """COCO's compressed run lengths (as its maskApi's rleFrString): 6 bits a character, 5 of
    them value, from the third on as differences from two before."""
    runs, p = [], 0
    while p < len(counts):
        x, k, more = 0, 0, True
        while more:
            c = ord(counts[p]) - 48
            x |= (c & 0x1F) << (5 * k)
            more = bool(c & 0x20)
            p += 1
            k += 1
            if not more and c & 0x10:
                x |= -1 << (5 * k)
        if len(runs) > 2:
            x += runs[-2]
        runs.append(x)
    return runs


def decode(counts, h, w):
    """A compressed mask, h x w (stored column by column, runs of 0 then 1 in turn)."""
    flat = np.zeros(h * w, dtype=np.uint8)
    at, value = 0, 0
    for n in run_lengths(counts):
        if value:
            flat[at : at + n] = 1
        at += n
        value ^= 1
    return flat.reshape(w, h).T


def body_label(size, people):
    """BODY for a picture `size` (w, h) from its DensePose people, as DensePose's own code draws
    them; IGNORE everywhere else."""
    w, h = size
    out = np.full((h, w), IGNORE, dtype=np.uint8)
    for (bx, by, bw, bh), masks in people:
        parts = np.zeros((256, 256), dtype=np.uint8)
        for i, counts in enumerate(masks):
            if counts:
                parts[decode(counts, 256, 256) > 0] = i + 1
        x0, y0 = int(bx), int(by)
        x1, y1 = min(int(bx + bw), w), min(int(by + bh), h)
        if x1 <= x0 or y1 <= y0:
            continue
        placed = np.asarray(Image.fromarray(parts).resize((int(bx + bw) - x0, int(by + bh) - y0), Image.NEAREST))[: y1 - y0, : x1 - x0]
        region = out[y0:y1, x0:x1]
        region[placed > 0] = DP_BODY[placed[placed > 0]]
    return Image.fromarray(out)


def ade_items(split, root):
    labels = sorted(glob.glob(f"{root}/annotations/{split}/*.png"))
    return [(p.replace("/annotations/", "/images/").replace(".png", ".jpg"), p, None, None, None) for p in labels]


ADE_LUT = np.full(256, OTHER, dtype=np.uint8)
ADE_LUT[0] = IGNORE


def ade_classes(root):
    """ADE20K's label values to SCENE, by the names in its objectInfo150.txt."""
    names = {}
    with open(f"{root}/objectInfo150.txt") as f:
        for line in f.readlines()[1:]:
            parts = line.rstrip("\n").split("\t")
            names[int(parts[0])] = parts[-1]
    for i, n in names.items():
        ADE_LUT[i] = scene_of(n)
    print("ADE20K:", {SCENE[k]: [names[i].split(",")[0] for i in names if ADE_LUT[i] == k] for k in range(1, len(SCENE))}, flush=True)
    assert ADE_LUT[1] == WALL and ADE_LUT[3] == SKY and ADE_LUT[13] == PERSON and ADE_LUT[4] == GROUND, "ADE20K's label numbers aren't what this expects"


# ATR's pictures and labels, as encoded bytes: [(picture, label)].
ATR_DATA = []


def atr_items():
    """ATR, all of it in memory (about 0.8 GB), as items for Pictures."""
    import pyarrow.parquet as pq
    from huggingface_hub import HfApi, hf_hub_download

    files = sorted(f for f in HfApi().list_repo_files(ATR, repo_type="dataset", revision=ATR_REVISION) if f.endswith(".parquet"))
    for name in files:
        table = pq.read_table(hf_hub_download(ATR, name, repo_type="dataset", revision=ATR_REVISION)).to_pydict()
        ATR_DATA.extend((i["bytes"], m["bytes"]) for i, m in zip(table["image"], table["mask"]))
    items = [("atr", i, None, None, None) for i in range(len(ATR_DATA))]
    return items[:-500], items[-500:]


# ---- CIHP, Danbooru-Parsing and skytnt's anime-segmentation ----------------------------------


def get_cihp():
    """CIHP from its authors' Google Drive (about 2 GB): Training and Validation, each with
    Images, Category_ids (the parts) and Human_ids (who's who)."""
    root = f"{CIHP}/instance-level_human_parsing"
    if not os.path.exists(f"{root}/Validation/Human_ids"):
        os.makedirs(CIHP, exist_ok=True)
        sh(f"curl -sSfL -o /tmp/cihp.tar.gz '{CIHP_URL}' && tar xzf /tmp/cihp.tar.gz -C {CIHP} && rm /tmp/cihp.tar.gz")
    return root


def cihp_items(root, split):
    names = sorted(os.path.basename(p)[:-4] for p in glob.glob(f"{root}/{split}/Human_ids/*.png"))
    return [("cihp", f"{root}/{split}/%s/{n}", None, None, None) for n in names]


def get_anime_faces():
    """Danbooru-Parsing from StyleAnime's Google Drive (about 200 MB)."""
    root = f"{ANIME_FACES}/Danbooru-Parsing"
    if not os.path.exists(f"{root}/anime_seg_test_68"):
        os.makedirs(ANIME_FACES, exist_ok=True)
        sh(f"curl -sSfL -o /tmp/faces.zip '{ANIME_FACES_URL}' && unzip -q -o /tmp/faces.zip -x '__MACOSX/*' '*.DS_Store' -d {ANIME_FACES} && rm /tmp/faces.zip")
    return root


def anime_face_items(root, split):
    labels = sorted(glob.glob(f"{root}/anime_seg_{split}/*.png"))
    return [("face", p, None, None, None) for p in labels if os.path.exists(p.replace("anime_seg_", "anime_face_")[:-4] + ".jpg")]


# skytnt's zips' members: [(zip, member)], read in each worker through its own open zips.
CUTOUT_FG, CUTOUT_BG, ZIPS = [], [], {}


def zip_read(path, member):
    # Each process its own: a zip opened before the DataLoader's workers fork would be shared.
    key = (os.getpid(), path)
    if key not in ZIPS:
        ZIPS[key] = zipfile.ZipFile(path)
    return ZIPS[key].read(member)


def get_anime_cutouts():
    """skytnt's anime-segmentation from Hugging Face (about 17.5 GB, left zipped): the cut-out
    characters and backgrounds, and its pictures with masks, unpacked. Returns the pictures as
    items: all but the last 300 for training, those for checking."""
    from huggingface_hub import hf_hub_download

    names = [f"data/fg-{i:02d}.zip" for i in range(6)] + [f"data/bg-{i:02d}.zip" for i in range(5)] + ["data/imgs-masks.zip"]
    for name in names:
        path = hf_hub_download(ANIME_CUTOUTS, name, repo_type="dataset", revision=ANIME_CUTOUTS_REVISION, local_dir=ANIME_CUTOUTS_DIR)
        if name.endswith("imgs-masks.zip"):
            if not os.path.exists(f"{ANIME_CUTOUTS_DIR}/masks"):
                with zipfile.ZipFile(path) as z:
                    z.extractall(ANIME_CUTOUTS_DIR)
            continue
        with zipfile.ZipFile(path) as z:
            members = [(path, m) for m in z.namelist() if m.lower().endswith((".png", ".jpg", ".jpeg", ".webp"))]
        (CUTOUT_FG if "/fg-" in name else CUTOUT_BG).extend(members)
    pictures = {os.path.splitext(e.name)[0]: e.path for e in os.scandir(f"{ANIME_CUTOUTS_DIR}/imgs")}
    items = []
    for m in sorted(glob.glob(f"{ANIME_CUTOUTS_DIR}/masks/*")):
        stem = os.path.splitext(os.path.basename(m))[0]
        if stem in pictures:
            items.append(("animepic", (pictures[stem], m), None, None, None))
    print(f"anime cut-outs: {len(CUTOUT_FG)} characters, {len(CUTOUT_BG)} backgrounds, {len(items)} pictures with masks", flush=True)
    return items[:-300], items[-300:]


def far_from(mask, fraction):
    """Where's further than about `fraction` of the picture's smaller side from `mask`."""
    h, w = mask.shape
    small = Image.fromarray(mask.astype(np.uint8) * 255).resize((64, max(1, round(64 * h / w))), Image.BILINEAR)
    near = small.filter(ImageFilter.MaxFilter(2 * max(1, round(64 * fraction * min(h, w) / w)) + 1))
    return np.asarray(near.resize((w, h), Image.BILINEAR)) < 64


def cutouts_picture(index):
    """One to three of the cut-out characters (the first `index`) over a background, at sizes
    from small to bigger than the picture (close-ups): the picture, [3][h][w] labels and who's
    who, at twice the labels' size."""
    cw, ch = WIDTH * 2, HEIGHT * 2
    path, member = random.choice(CUTOUT_BG)
    bg = Image.open(io.BytesIO(zip_read(path, member)))
    bg.draft("RGB", (cw, ch))
    bg = bg.convert("RGB")
    w, h = bg.size
    s = random.uniform(0.5, 1.0)
    bw = min(w, h * cw / ch) * s
    bh = bw * ch / cw
    x, y = random.uniform(0, w - bw), random.uniform(0, h - bh)
    canvas = bg.resize((cw, ch), Image.BILINEAR, box=(x, y, min(x + bw, w), min(y + bh, h)))
    who = np.zeros((ch, cw), dtype=np.uint8)
    picks = [index] + [random.randrange(len(CUTOUT_FG)) for _ in range(random.choices([0, 1, 2], [0.5, 0.3, 0.2])[0])]
    for k, i in enumerate(picks, 1):
        fg = Image.open(io.BytesIO(zip_read(*CUTOUT_FG[i]))).convert("RGBA")
        box = fg.getchannel("A").point(lambda a: 255 if a > 127 else 0).getbbox()
        if not box:
            continue
        fg = fg.crop(box)
        tall = ch * (random.uniform(0.3, 1.0) if random.random() < 0.6 else random.uniform(1.0, 2.0))
        scale = tall / fg.height
        fg = fg.resize((max(1, round(fg.width * scale)), max(1, round(fg.height * scale))), Image.BILINEAR)
        fw, fh = fg.size
        px = round(random.uniform(-0.3 * fw, cw - 0.7 * fw))
        # Taller than the picture: their top (as a close-up's head and shoulders) is in it.
        py = round(-random.uniform(0, 0.15) * fh) if fh > ch else round(random.uniform(0, ch - fh))
        canvas.paste(fg, (px, py), fg)
        alpha = np.zeros((ch, cw), dtype=bool)
        x0, y0, x1, y1 = max(px, 0), max(py, 0), min(px + fw, cw), min(py + fh, ch)
        if x1 > x0 and y1 > y0:
            alpha[y0:y1, x0:x1] = np.asarray(fg.getchannel("A"))[y0 - py : y1 - py, x0 - px : x1 - px] > 127
        who[alpha] = k
    scene = np.where(who > 0, PERSON, NOT_PERSON).astype(np.uint8)
    unknown = np.full_like(scene, IGNORE)
    return canvas, np.stack([scene, unknown, unknown]), who


def new_source(kind, where, index):
    """CIHP's, Danbooru-Parsing's and skytnt's pictures: the picture, [3][h][w] labels (SCENE,
    PARTS, BODY) and who's who (0 nobody, each person a number), None if it isn't known; and
    where the heat of CENTRES counts (None: wherever SCENE is known)."""
    if kind == "cihp":
        img = Image.open(where % "Images" + ".jpg").convert("RGB")
        a = np.asarray(Image.open(where % "Category_ids" + ".png"))
        who = np.asarray(Image.open(where % "Human_ids" + ".png")).astype(np.uint8)
        # Its background doesn't count for CENTRES either: some people there aren't labelled.
        return img, np.stack([CIHP_SCENE[a], CIHP_PART[a], CIHP_BODY[a]]), who, None
    if kind == "face":
        img = Image.open(where.replace("anime_seg_", "anime_face_")[:-4] + ".jpg").convert("RGB")
        a = np.asarray(Image.open(where).convert("L"))
        person = AF_PERSON[a]
        scene = np.where(person, PERSON, np.where(far_from(person, 0.12), NOT_PERSON, IGNORE)).astype(np.uint8)
        return img, np.stack([scene, AF_PART[a], AF_BODY[a]]), person.astype(np.uint8), None
    if kind == "animepic":
        img = Image.open(where[0]).convert("RGB")
        person = np.asarray(Image.open(where[1]).convert("L").resize(img.size, Image.NEAREST)) > 127
        scene = np.where(person, PERSON, NOT_PERSON).astype(np.uint8)
        unknown = np.full_like(scene, IGNORE)
        # Several characters share one mask: who's who isn't known.
        return img, np.stack([scene, unknown, unknown]), None, None
    if kind == "cutouts":
        img, labels, who = cutouts_picture(index)
        return img, labels, who, None
    raise ValueError(kind)


def person_boxes(who):
    """Each person's box (x0, y0, x1, y1) in who's who."""
    boxes = []
    for k in np.unique(who):
        if k:
            ys, xs = np.nonzero(who == k)
            boxes.append((xs.min(), ys.min(), xs.max() + 1, ys.max() + 1))
    return boxes


def crop_box(size, boxes, train):
    """The part of a picture (w, h) the model sees: the largest of its shape when checking; when
    training, smaller at random, or (CLOSE_UPS of the time, if anyone is big enough) closed in
    on someone as a film's close-ups and medium shots are, never upscaled by more than 1/0.75."""
    w, h = size
    cw = min(w, h * WIDTH / HEIGHT)
    ch = cw * HEIGHT / WIDTH
    if not train:
        x, y = (w - cw) / 2, (h - ch) / 2
    else:
        least = HEIGHT * 0.75
        big = [b for b in boxes if b[3] - b[1] >= least]
        if big and random.random() < CLOSE_UPS:
            x0, y0, x1, y1 = random.choice(big)
            ch = min(ch, max(least, (y1 - y0) * random.uniform(0.3, 1.1)))
            cw = ch * WIDTH / HEIGHT
            if cw > w:
                cw, ch = w, w * HEIGHT / WIDTH
            x = (x0 + x1) / 2 - cw / 2 + random.uniform(-0.3, 0.3) * cw
            y = y0 - random.uniform(0.05, 0.25) * ch
            x, y = min(max(x, 0.0), w - cw), min(max(y, 0.0), h - ch)
        else:
            s = random.uniform(0.5, 1.0)
            cw, ch = cw * s, ch * s
            x, y = random.uniform(0, w - cw), random.uniform(0, h - ch)
    # cw * HEIGHT / WIDTH can round to a hair over the picture's size.
    x, y = max(x, 0.0), max(y, 0.0)
    return (x, y, min(x + cw, w), min(y + ch, h))


def to_classes(label, ids, cls, body):
    """SCENE, PARTS and BODY labels for a label image, [3][h][w], and each person's pixels (as a
    list of masks), or None where who's who isn't known. `body`: DensePose's BODY labels, or
    None."""
    a = np.asarray(label)
    unknown = np.full(a.shape[:2], IGNORE, dtype=np.uint8)
    if isinstance(ids, str):
        scene = ATR_SCENE[a]
        return np.stack([scene, ATR_PART[a], ATR_BODY[a]]), [scene == PERSON]
    if ids is None:
        return np.stack([ADE_LUT[a], unknown, unknown]), None
    # COCO: each pixel's colour is a segment's id. Id 0 and anything unlisted is unlabelled.
    v = a[..., 0].astype(np.int64) + a[..., 1].astype(np.int64) * 256 + a[..., 2].astype(np.int64) * 65536
    at = np.minimum(np.searchsorted(ids, v), len(ids) - 1)
    out = cls[at].copy()
    out[ids[at] != v] = IGNORE
    people = [v == i for i, c in zip(ids, cls) if c == PERSON]
    body = unknown if body is None else np.asarray(body)
    return np.stack([out, unknown, body]), [m for m in people if m.any()]


YS, XS = np.mgrid[0:HEIGHT, 0:WIDTH].astype(np.float32)


def centres(scene, people, counted=None):
    """CENTRES' targets for one picture: [5][h][w]: the Gaussians, x and y to the centre (in
    picture heights), and how much each pixel counts for the first and for the other two. A
    crowd's pixels (labelled IGNORE in SCENE, as COCO's crowds are) don't count for the first,
    unless `counted` says where it counts."""
    t = np.zeros((5, HEIGHT, WIDTH), dtype=np.float32)
    if people is None:
        return t
    t[3] = scene != IGNORE if counted is None else counted
    for m in people:
        if not m.any():
            continue  # cropped out
        cy, cx = YS[m].mean(), XS[m].mean()
        t[0] = np.maximum(t[0], np.exp(-((XS - cx) ** 2 + (YS - cy) ** 2) / (2 * CENTRE_SIGMA**2)))
        t[1][m] = (cx - XS[m]) / HEIGHT
        t[2][m] = (cy - YS[m]) / HEIGHT
        t[4][m] = 1.0
    return t


def grade(img):
    """Differences between snapshots and what films look like: exposure, colour grading,
    softness and compression."""
    img = ImageEnhance.Brightness(img).enhance(random.uniform(0.6, 1.3))
    img = ImageEnhance.Contrast(img).enhance(random.uniform(0.7, 1.3))
    img = ImageEnhance.Color(img).enhance(random.uniform(0.5, 1.4))
    gamma = random.uniform(0.8, 1.25)
    lut = []
    for _ in range(3):
        gain = random.uniform(0.9, 1.1)
        lut += [min(255, int(255 * (i / 255) ** gamma * gain)) for i in range(256)]
    img = img.point(lut)
    if random.random() < 0.25:
        img = img.filter(ImageFilter.GaussianBlur(random.uniform(0.3, 1.5)))
    if random.random() < 0.3:
        buf = io.BytesIO()
        img.save(buf, "JPEG", quality=random.randint(20, 70))
        img = Image.open(io.BytesIO(buf.getvalue())).convert("RGB")
    return img


class Pictures(Dataset):
    def __init__(self, items, train):
        self.items, self.train = items, train

    def __len__(self):
        return len(self.items)

    @staticmethod
    def boxes(label, ids, cls):
        """The people's boxes in a COCO or ATR label image, for close-ups (ADE20K's people aren't
        told apart)."""
        if ids is None:
            return []
        a = np.asarray(label)
        if isinstance(ids, str):
            return person_boxes((ATR_SCENE[a] == PERSON).astype(np.uint8))
        v = a[..., 0].astype(np.int64) + a[..., 1].astype(np.int64) * 256 + a[..., 2].astype(np.int64) * 65536
        who = np.zeros(v.shape, dtype=np.uint8)
        for k, i in enumerate([i for i, c in zip(ids, cls) if c == PERSON][:255], 1):
            who[v == i] = k
        return person_boxes(who)

    def new_source(self, kind, where, i):
        img, labels, who, counted = new_source(kind, where, i)
        # Danbooru-Parsing's are close-ups already, and the cut-outs are put together at all sizes.
        box = crop_box(img.size, person_boxes(who) if kind == "cihp" and self.train else [], self.train)
        img = img.resize((WIDTH, HEIGHT), Image.BICUBIC, box=box)

        def cut(a):
            return np.asarray(Image.fromarray(a).resize((WIDTH, HEIGHT), Image.NEAREST, box=box))

        labels = np.stack([cut(a) for a in labels])
        who = None if who is None else cut(who)
        counted = None if counted is None else cut(counted.astype(np.uint8)) > 0
        if self.train:
            if random.random() < 0.5:
                img = img.transpose(Image.FLIP_LEFT_RIGHT)
                labels = labels[:, :, ::-1]
                who = None if who is None else who[:, ::-1]
                counted = None if counted is None else counted[:, ::-1]
            img = grade(img)
        rgb = torch.from_numpy(np.array(img)).permute(2, 0, 1)
        people = None if who is None else [who == k for k in np.unique(who) if k]
        t = centres(labels[0], people, counted)
        return rgb, torch.from_numpy(labels.copy()).long(), torch.from_numpy(t)

    def __getitem__(self, i):
        path, label_path, ids, cls, people = self.items[i]
        if path in ("cihp", "face", "animepic", "cutouts"):
            return self.new_source(path, label_path, i)
        if path == "atr":
            picture, mask = ATR_DATA[label_path]
            img = Image.open(io.BytesIO(picture)).convert("RGB")
            label = Image.open(io.BytesIO(mask))
            ids = "atr"
        else:
            img = Image.open(path).convert("RGB")
            label = Image.open(label_path)
        body = body_label(img.size, people) if people else None
        box = crop_box(img.size, self.boxes(label, ids, cls) if self.train else [], self.train)
        img = img.resize((WIDTH, HEIGHT), Image.BICUBIC, box=box)
        label = label.resize((WIDTH, HEIGHT), Image.NEAREST, box=box)
        if body is not None:
            body = body.resize((WIDTH, HEIGHT), Image.NEAREST, box=box)
        if self.train:
            if random.random() < 0.5:
                img = img.transpose(Image.FLIP_LEFT_RIGHT)
                label = label.transpose(Image.FLIP_LEFT_RIGHT)
                if body is not None:
                    body = body.transpose(Image.FLIP_LEFT_RIGHT)
            img = grade(img)
        rgb = torch.from_numpy(np.array(img)).permute(2, 0, 1)
        classes, who = to_classes(label, ids, cls, body)
        return rgb, torch.from_numpy(classes.copy()).long(), torch.from_numpy(centres(classes[0], who))


class Mix(Sampler):
    """Forever: items from consecutive datasets of `sizes`, each picked with its share of
    `weights` however big it is."""

    def __init__(self, sizes, weights, seed=0):
        self.sizes, self.weights, self.seed = sizes, weights, seed

    def __iter__(self):
        r = random.Random(self.seed)
        starts = np.cumsum([0] + list(self.sizes))
        while True:
            k = r.choices(range(len(self.sizes)), self.weights)[0]
            yield int(starts[k]) + r.randrange(self.sizes[k])

    def __len__(self):
        return 1 << 40


# ---- Model -----------------------------------------------------------------------------------


class Head(nn.Module):
    """Every step here has a kernel in depth.wgsl, with bilinear resizes as align_corners."""

    def __init__(self):
        super().__init__()
        self.token9 = nn.Conv2d(384, 128, 1)
        self.token12 = nn.Conv2d(384, 128, 1)
        self.context = nn.Conv2d(128, 128, 3, padding=1)
        self.mid = nn.Conv2d(128, 64, 3, padding=1)
        self.fused = nn.Conv2d(64, 64, 1)
        # Wider at the end than the old Depth Anything head's (48), for 25 answers instead of four.
        self.fine1 = nn.Conv2d(64, 64, 3, padding=1)
        self.fine2 = nn.Conv2d(64, 64, 3, padding=1)
        self.out = nn.Conv2d(64, OUTPUTS, 1)

    def forward(self, t9, t12, fused):
        x = F.relu(self.context(F.relu(self.token9(t9) + self.token12(t12))))
        x = F.interpolate(x, size=(GH * 4, GW * 4), mode="bilinear", align_corners=True)
        x = F.relu(self.mid(x))
        x = F.interpolate(x, size=fused.shape[2:], mode="bilinear", align_corners=True)
        x = F.relu(self.fine1(F.relu(x + self.fused(fused))))
        return self.out(F.relu(self.fine2(x)))


def grid(tokens):
    """Tokens without the class token, as an image."""
    b = tokens.shape[0]
    return tokens.transpose(1, 2).reshape(b, 384, GH, GW).float()


def load_moge():
    """MoGe-2 from its own code (with the libraries it needs only for other things stood in
    for) and its weights from Hugging Face, at the versions flex-depth was checked against."""
    import importlib.abc
    import importlib.machinery
    import sys
    import types

    class Absent(importlib.abc.MetaPathFinder, importlib.abc.Loader):
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

    if not os.path.exists(f"{MOGE_DIR}/moge"):
        sh(f"git clone -q {MOGE_CODE} {MOGE_DIR} && git -C {MOGE_DIR} checkout -q {MOGE_COMMIT}")
    sys.meta_path.insert(0, Absent())
    sys.path.insert(0, MOGE_DIR)
    from huggingface_hub import hf_hub_download
    from moge.model.v2 import MoGeModel

    return MoGeModel.from_pretrained(hf_hub_download(MODEL, "model.pt", revision=REVISION))


@torch.no_grad()
def features(depth, rgb):
    """What flex-depth's MoGe pass has: the two taps' normed tokens, and the neck's level 3."""
    x = rgb.float() / 255
    enc = depth.encoder
    with torch.autocast(device, enabled=device == "cuda"):
        x14 = (x - enc.image_mean) / enc.image_std
        taps = enc.backbone.get_intermediate_layers(x14, n=enc.intermediate_layers, return_class_token=True)
        projected = sum(proj(t.permute(0, 2, 1).unflatten(2, (GH, GW)).contiguous()) for proj, (t, _) in zip(enc.output_projections, taps))
        neck_in = [projected, None, None, None, None]
        for level in range(5):
            uv = normalized_view_plane_uv(width=GW * 2**level, height=GH * 2**level, aspect_ratio=GW / GH, dtype=x.dtype, device=x.device)
            uv = uv.permute(2, 0, 1).unsqueeze(0).expand(x.shape[0], -1, -1, -1)
            neck_in[level] = uv if neck_in[level] is None else torch.cat([neck_in[level], uv], 1)
        fine = depth.neck(neck_in)[3]
    assert fine.shape[1:] == (64, GH * 8, GW * 8), fine.shape
    return grid(taps[0][0]), grid(taps[1][0]), fine.float()


def logits_of(head, feats):
    return F.interpolate(head(*feats), size=(HEIGHT, WIDTH), mode="bilinear", align_corners=True)


def part_of(logits, label, classes, weights):
    """Cross-entropy keeps soft edges honest; Dice on every class but the first keeps small
    things from being lost in the big ones. Nothing where nothing is labelled."""
    known = label != IGNORE
    if not known.any():
        return logits.sum() * 0.0
    ce = F.cross_entropy(logits, label, weight=weights, ignore_index=IGNORE)
    p = logits.softmax(1)
    dice = 0.0
    for c in classes:
        want = (label == c) & known
        inter = (p[:, c] * want).sum()
        dice = dice + 1 - (2 * inter + 1) / ((p[:, c] * known).sum() + want.sum() + 1)
    return ce + dice / len(classes)


def loss_of(logits, y, t):
    """`y` is [batch][3][h][w]: SCENE's labels, then PARTS' and BODY's (on a person's pixels);
    `t` CENTRES' targets (see `centres`)."""
    n1, n2, n3 = len(SCENE), len(SCENE) + len(PARTS), len(CLASSES)
    # NOT_PERSON: anything but PERSON, as -log(1 - p(person)).
    nobody = y[:, 0] == NOT_PERSON
    loss = part_of(logits[:, :n1], y[:, 0].masked_fill(nobody, IGNORE), range(1, len(SCENE)), SCENE_WEIGHTS.to(y.device))
    if nobody.any():
        others = [c for c in range(n1) if c != PERSON]
        not_person = torch.logsumexp(logits[:, others], 1) - torch.logsumexp(logits[:, :n1], 1)
        loss = loss - not_person[nobody].mean()
    loss = loss + part_of(logits[:, n1:n2], y[:, 1], range(len(PARTS)), None)
    loss = loss + part_of(logits[:, n2:n3], y[:, 2], range(len(BODY)), None)
    heat, offsets = logits[:, n3], logits[:, n3 + 1 : n3 + 3]
    loss = loss + CENTRE_WEIGHT * (((heat - t[:, 0]) ** 2) * t[:, 3]).sum() / t[:, 3].sum().clamp(min=1)
    near = t[:, 4:5]
    return loss + OFFSET_WEIGHT * ((offsets - t[:, 1:3]).abs() * near).sum() / (2 * near.sum()).clamp(min=1)


# ---- Checking --------------------------------------------------------------------------------


def check_labels(name, items):
    """How much of the pictures each class covers, and how many people there are a picture: a
    wrong mapping shows up here, cheaply."""
    counts = np.zeros(len(CLASSES), dtype=np.int64)
    people, pictures = 0.0, 0
    data = Pictures(items[:: max(1, len(items) // 300)], False)
    for _, y, t in DataLoader(data, batch_size=16, num_workers=WORKERS):
        for k, (start, names) in enumerate([(0, SCENE), (len(SCENE), PARTS), (len(SCENE) + len(PARTS), BODY)]):
            for c in range(len(names)):
                counts[start + c] += int((y[:, k] == c).sum())
        people += len(peaks(t[:, 0], 0.99))
        pictures += len(y)
    share = counts / max(1, counts[: len(SCENE)].sum())
    print(name, {c: round(float(s), 3) for c, s in zip(CLASSES, share)}, f"{people / max(pictures, 1):.1f} people a picture", flush=True)
    return share


def peaks(heat, threshold=0.3):
    """Each picture's centres: [(picture, y, x)] where the Gaussians peak above `threshold`,
    strongest first, and none within 3 pixels of a stronger one (a centre between pixels
    peaks at several)."""
    top = F.max_pool2d(heat[:, None], 7, 1, 3)[:, 0]
    found = ((heat == top) & (heat > threshold)).nonzero().tolist()
    found.sort(key=lambda p: -float(heat[p[0], p[1], p[2]]))
    kept = []
    for b, y, x in found:
        if not any(b == b2 and (y - y2) ** 2 + (x - x2) ** 2 <= 9 for b2, y2, x2 in kept):
            kept.append((b, y, x))
    return kept


@torch.no_grad()
def evaluate(depth, head, items, which):
    """IoU of each of SCENE's classes but "other", of PARTS's or of BODY's (`which` 0, 1 or 2);
    or, with `which` 3, how many of the people's centres are found (F1, matched within 8
    pixels) and how much of their pixels point within 6 pixels of their centre; or, with
    `which` 4, PERSON's IoU where it's known to be a person or NOT_PERSON."""
    head.eval()
    names = [SCENE, PARTS, BODY][which] if which < 3 else ["person"] if which == 4 else []
    starts = [0, len(SCENE), len(SCENE) + len(PARTS), len(CLASSES)]
    inter, union = np.zeros(len(names)), np.zeros(len(names))
    found = wanted = guessed = 0
    near = person = 0.0
    for rgb, y, t in DataLoader(Pictures(items, False), batch_size=BATCH, num_workers=WORKERS):
        rgb, y, t = rgb.to(device), y.to(device), t.to(device)
        logits = logits_of(head, features(depth, rgb))
        if which == 4:
            p, y = logits[:, : len(SCENE)].argmax(1) == PERSON, y[:, 0]
            known = (y == PERSON) | (y == NOT_PERSON)
            inter[0] += int((p & (y == PERSON) & known).sum())
            union[0] += int(((p | (y == PERSON)) & known).sum())
            continue
        if which == 3:
            out = logits[:, starts[3] :]
            mine, theirs = peaks(out[:, 0]), peaks(t[:, 0], 0.99)
            guessed += len(mine)
            wanted += len(theirs)
            # One to one: each peak (strongest first) takes the nearest centre not yet taken.
            left = list(theirs)
            for b, py, px in mine:
                d = [((py - y2) ** 2 + (px - x2) ** 2, j) for j, (b2, y2, x2) in enumerate(left) if b2 == b]
                if d and min(d)[0] <= 64:
                    left.pop(min(d)[1])
                    found += 1
            miss = ((out[:, 1:3] - t[:, 1:3]) * HEIGHT).norm(dim=1)
            on = t[:, 4]
            near += float(((miss <= 6) * on).sum())
            person += float(on.sum())
            continue
        p, y = logits[:, starts[which] : starts[which + 1]].argmax(1), y[:, which]
        known = y != IGNORE
        for c in range(len(names)):
            a, b = (p == c) & known, (y == c) & known
            inter[c] += int((a & b).sum())
            union[c] += int((a | b).sum())
    head.train()
    if which == 3:
        precision, recall = found / max(guessed, 1), found / max(wanted, 1)
        return {"centres found": round(2 * precision * recall / max(precision + recall, 1e-9), 4), "pixels to their centre": round(near / max(person, 1), 4)}
    return {c: round(float(i / max(u, 1)), 4) for c, i, u in zip(names, inter, union) if c != "other"}


@torch.no_grad()
def save_samples(depth, head, items, path):
    """Each picture beside its classes; a person's parts over their person; their body parts;
    and who's who (each person's pixels coloured by the centre they point to, centres white)."""
    head.eval()
    data = Pictures(items, False)
    rows = []
    n1, n2, n3 = len(SCENE), len(SCENE) + len(PARTS), len(CLASSES)
    for i in range(0, len(items), max(1, len(items) // SAMPLES)):
        rgb = data[i][0]
        logits = logits_of(head, features(depth, rgb[None].to(device)))[0]
        scene = logits[:n1].softmax(0).cpu().numpy()
        parts = logits[n1:n2].softmax(0).cpu().numpy() * scene[PERSON]
        body = logits[n2:n3].softmax(0).cpu().numpy() * scene[PERSON]
        picture = rgb.permute(1, 2, 0).numpy().astype(np.float32)
        tint = (COLOURS[:, None, None] * scene[:, :, :, None]).sum(0)
        alpha = 1.0 - scene[OTHER]
        over = picture * (1 - 0.6 * alpha[:, :, None]) + tint * 0.6
        on = parts.sum(0)[:, :, None]
        people = picture * (1 - 0.7 * on) + (PART_COLOURS[:, None, None] * parts[:, :, :, None]).sum(0) * 0.7
        limbs = picture * (1 - 0.7 * on) + (BODY_COLOURS[:, None, None] * body[:, :, :, None]).sum(0) * 0.7
        out = logits[n3:].cpu()
        found = [(py, px) for _, py, px in peaks(out[0:1])]
        who = picture * 0.3
        if found:
            ty = YS + out[2].numpy() * HEIGHT
            tx = XS + out[1].numpy() * HEIGHT
            d = np.stack([(ty - py) ** 2 + (tx - px) ** 2 for py, px in found])
            nearest = d.argmin(0)
            hues = np.array([[(97 * k) % 256, (57 * k + 120) % 256, (151 * k + 60) % 256] for k in range(len(found))], dtype=np.float32)
            mask = (scene[PERSON] > 0.5)[:, :, None]
            who = np.where(mask, hues[nearest] * 0.8 + picture * 0.2, who)
            for py, px in found:
                who[max(py - 2, 0) : py + 3, max(px - 2, 0) : px + 3] = 255
        rows.append(np.concatenate([picture, over, people, limbs, who], 1))
        if len(rows) == SAMPLES:
            break
    head.train()
    Image.fromarray(np.concatenate(rows, 0).clip(0, 255).astype(np.uint8)).save(path, quality=85)


def main():
    os.makedirs(OUT, exist_ok=True)
    sh("nvidia-smi --query-gpu=name,memory.total --format=csv || true; nproc; df -h /tmp | tail -1")
    # The new sets download alongside the others (CIHP's Google Drive can be slow).
    from concurrent.futures import ThreadPoolExecutor

    pool = ThreadPoolExecutor(3)
    fetching = [pool.submit(get_cihp), pool.submit(get_anime_faces), pool.submit(get_anime_cutouts)]
    if os.environ.get("SEG_FETCH"):
        fetch_data()
    get_panoptic()
    coco = find("coco2017")
    ade = find("ADEChallengeData2016")
    coco_train, coco_val = coco_items("train2017", f"{coco}/train2017"), coco_items("val2017", f"{coco}/val2017")
    ade_train, ade_val = ade_items("training", ade), ade_items("validation", ade)
    ade_classes(ade)
    atr_train, atr_val = atr_items()
    # DensePose's people on COCO's pictures. Its minival set is kept for scoring: those pictures
    # are in COCO's training or validation set, and only lose their bodies for training.
    taught = densepose(os.environ.get("SEG_DENSEPOSE_TRAIN", "densepose_coco_2014_train.json,densepose_coco_2014_valminusminival.json").split(","))
    kept = densepose([os.environ.get("SEG_DENSEPOSE_VAL", "densepose_coco_2014_minival.json")])
    coco_train = [(*it[:4], taught.get(image_id(it))) for it in coco_train]
    dp_train = [it for it in coco_train if it[4]]
    dp_val = [(*it[:4], kept[image_id(it)]) for it in coco_train + coco_val if image_id(it) in kept]
    cihp, faces, (anime_train, anime_val) = (f.result() for f in fetching)
    cihp_train, cihp_val = cihp_items(cihp, "Training"), cihp_items(cihp, "Validation")[::4]
    face_train, face_val = anime_face_items(faces, "train"), anime_face_items(faces, "test_68")
    cutouts = [("cutouts", None, None, None, None)] * len(CUTOUT_FG)
    print(f"COCO {len(coco_train)} + {len(coco_val)}, ADE20K {len(ade_train)} + {len(ade_val)}, ATR {len(atr_train)} + {len(atr_val)}, "
          f"DensePose {len(dp_train)} + {len(dp_val)}, CIHP {len(cihp_train)} + {len(cihp_val)}, anime faces {len(face_train)} + {len(face_val)}, "
          f"anime pictures {len(anime_train)} + {len(anime_val)}, anime cut-outs {len(cutouts)} pictures", flush=True)
    assert coco_train and coco_val and ade_train and ade_val and atr_train and dp_train and dp_val
    assert cihp_train and cihp_val and face_train and face_val and anime_train and anime_val and CUTOUT_FG and CUTOUT_BG
    if os.environ.get("SEG_LIMIT"):
        n = int(os.environ["SEG_LIMIT"])
        (coco_train, coco_val, ade_train, ade_val, atr_train, atr_val, dp_train, dp_val, cihp_train, cihp_val, face_train, face_val,
         anime_train, anime_val, cutouts) = (
            x[:: max(1, len(x) // n)][:n] for x in (coco_train, coco_val, ade_train, ade_val, atr_train, atr_val, dp_train, dp_val,
                                                    cihp_train, cihp_val, face_train, face_val, anime_train, anime_val, cutouts)
        )

    check_labels("COCO", coco_train)
    share = check_labels("ADE20K", ade_train)
    assert share[WALL] > share[SKY] > share[PERSON] > 0, "ADE20K's label numbers aren't what this expects"
    share = check_labels("ATR", atr_train)
    assert share[len(SCENE) + CLOTHES] > share[len(SCENE) + SKIN] > share[len(SCENE) + HAIR] > 0, "ATR's labels aren't what this expects"
    share = check_labels("DensePose", dp_train)
    b = len(SCENE) + len(PARTS)
    assert share[b + TORSO] > share[b + HEAD] > 0 and share[b + LEG] > share[b + FOOT] > 0 and share[b + ARM] > share[b + HAND] > 0, "DensePose's parts aren't what this expects"
    share = check_labels("CIHP", cihp_train)
    p = len(SCENE)
    assert share[p + CLOTHES] > share[p + SKIN] > 0 and share[p + HAIR] > 0 and share[b + HEAD] > share[b + FOOT] > 0, "CIHP's labels aren't what this expects"
    share = check_labels("anime faces", face_train)
    assert share[p + HAIR] > share[p + SKIN] > share[p + CLOTHES] > 0 and share[b + HEAD] > 0, "Danbooru-Parsing's labels aren't what this expects"
    check_labels("anime pictures", anime_train)
    check_labels("anime cut-outs", cutouts)

    depth = load_moge().eval().to(device).requires_grad_(False)
    global normalized_view_plane_uv
    from moge.utils.geometry_torch import normalized_view_plane_uv
    head = Head().to(device)
    params = sum(p.numel() for p in head.parameters())
    print(f"head: {params} weights, {params * 4 / 1e6:.2f} MB", flush=True)
    opt = torch.optim.AdamW(head.parameters(), lr=LR, weight_decay=WEIGHT_DECAY)
    # A run restarted after its machine was taken back (as Modal may) carries on from its last
    # checkpoint. SEG_RUN names the run, so another run's checkpoint is never picked up.
    run_id, resume_path, resumed = os.environ.get("SEG_RUN", ""), f"{OUT}/resume.pt", None
    if run_id and os.path.exists(resume_path):
        saved = torch.load(resume_path, map_location=device, weights_only=False)
        if saved.get("run") == run_id:
            resumed = saved
            head.load_state_dict(saved["head"])
            opt.load_state_dict(saved["opt"])

    sources = [coco_train, dp_train, ade_train, atr_train, cihp_train, face_train, anime_train, cutouts]
    data = torch.utils.data.ConcatDataset([Pictures(items, True) for items in sources])
    loader = DataLoader(
        data, batch_size=BATCH, sampler=Mix([len(items) for items in sources], MIX, resumed["step"] if resumed else 0), num_workers=WORKERS,
        pin_memory=device == "cuda", persistent_workers=True, prefetch_factor=4,
    )

    began = time.time()
    total = MINUTES * 60 - (began - START)
    print(f"setup took {(began - START) / 60:.1f} min, training for {total / 60:.0f} min", flush=True)
    history, best, step, running, shown, skipped = [], -1.0, 0, 0.0, time.time(), 0
    if resumed:
        total, trained = resumed["total"], resumed["trained"]
        began -= trained
        history, best, step = resumed["history"], resumed["best"], resumed["step"]
        print(f"carrying on run {run_id} from step {step}: {trained / 60:.0f} of {total / 60:.0f} min trained", flush=True)
    next_eval = time.time() + EVAL_MINUTES * 60

    def checkpoint():
        nonlocal best
        # Half of COCO's validation pictures, to keep this quick.
        scores = {
            "COCO": evaluate(depth, head, coco_val[::2], 0),
            "ADE20K": evaluate(depth, head, ade_val, 0),
            "ATR": evaluate(depth, head, atr_val, 1),
            "DensePose": evaluate(depth, head, dp_val[::2], 2),
            "people": evaluate(depth, head, coco_val[::2], 3),
            "CIHP": evaluate(depth, head, cihp_val, 1),
            "CIHP body": evaluate(depth, head, cihp_val, 2),
            "CIHP people": evaluate(depth, head, cihp_val, 3),
            "anime faces": evaluate(depth, head, face_val, 1),
            "anime people": evaluate(depth, head, anime_val, 4),
        }
        score = float(np.mean([np.mean(list(s.values())) for s in scores.values()]))
        # The first run's score (its five sets), to compare with.
        first = float(np.mean([np.mean(list(scores[k].values())) for k in ("COCO", "ADE20K", "ATR", "DensePose", "people")]))
        minutes = (time.time() - began) / 60
        history.append({"step": step, "minutes": round(minutes, 1), "score": round(score, 4), "first run's score": round(first, 4), **scores})
        print(f"step {step} ({minutes:.0f} min): score {score:.4f} (first run's {first:.4f}) {scores}", flush=True)
        # Written whole then renamed: the volume is saved while this runs.
        with open(f"{OUT}/metrics.json.part", "w") as f:
            json.dump(history, f, indent=1)
        os.replace(f"{OUT}/metrics.json.part", f"{OUT}/metrics.json")
        save_samples(depth, head, coco_val, f"{OUT}/samples_{step:07d}_coco.jpg")
        save_samples(depth, head, ade_val, f"{OUT}/samples_{step:07d}_ade.jpg")
        save_samples(depth, head, atr_val, f"{OUT}/samples_{step:07d}_atr.jpg")
        save_samples(depth, head, dp_val, f"{OUT}/samples_{step:07d}_densepose.jpg")
        save_samples(depth, head, cihp_val, f"{OUT}/samples_{step:07d}_cihp.jpg")
        save_samples(depth, head, face_val, f"{OUT}/samples_{step:07d}_anime_faces.jpg")
        save_samples(depth, head, anime_val, f"{OUT}/samples_{step:07d}_anime.jpg")
        if score > best:
            best = score
            save_file({k: v.cpu().contiguous() for k, v in head.state_dict().items()}, f"{OUT}/head.safetensors.part", {
                "scene": ",".join(SCENE), "parts": ",".join(PARTS), "body": ",".join(BODY), "centres": ",".join(CENTRES),
                "offsets": "picture heights", "model": MODEL, "revision": REVISION,
            })
            os.replace(f"{OUT}/head.safetensors.part", f"{OUT}/head.safetensors")
            print("best so far: saved", flush=True)
        if run_id:
            torch.save({
                "run": run_id, "head": head.state_dict(), "opt": opt.state_dict(), "step": step,
                "trained": time.time() - began, "total": total, "history": history, "best": best,
            }, f"{resume_path}.part")
            os.replace(f"{resume_path}.part", resume_path)

    for rgb, y, t in loader:
        now = time.time()
        frac = (now - began) / max(total, 1)
        if frac >= 1:
            break
        lr = LR * 0.5 * (1 + math.cos(math.pi * frac)) * min(1.0, (step + 1) / WARMUP)
        for g in opt.param_groups:
            g["lr"] = lr
        rgb, y, t = rgb.to(device, non_blocking=True), y.to(device, non_blocking=True), t.to(device, non_blocking=True)
        loss = loss_of(logits_of(head, features(depth, rgb)), y, t)
        if not torch.isfinite(loss):
            skipped += 1
            assert skipped <= 20, "the loss keeps coming out as NaN or infinite"
            print(f"step {step}: loss {loss.item()}, batch skipped", flush=True)
            continue
        opt.zero_grad(set_to_none=True)
        loss.backward()
        opt.step()
        step += 1
        running += loss.item()
        if step % 100 == 0:
            print(f"step {step} loss {running / 100:.4f} lr {lr:.2e} {100 * BATCH / (time.time() - shown):.0f} pictures/s", flush=True)
            running, shown = 0.0, time.time()
        if now >= next_eval:
            checkpoint()
            next_eval = time.time() + EVAL_MINUTES * 60
    checkpoint()
    print(f"done: best score {best:.4f}", flush=True)
    # The workers wait forever for the endless sampler otherwise.
    os._exit(0)


if __name__ == "__main__":
    main()
