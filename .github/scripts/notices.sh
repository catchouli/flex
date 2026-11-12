# What package.sh (Windows) and package-linux.sh have in common: the networks' licenses, and the
# notices for everything both packages bundle or download. Sourced by both, which set `fail`.

# What models/<name>.LICENSE is called in the package's licenses folder.
model_license_name() {
    case $(basename "$1" .LICENSE) in
    flow) echo neuflow ;;
    sound) echo yamnet ;;
    *) basename "$1" .LICENSE | tr _ - ;;
    esac
}

# copy_model_licenses <repo root> <licenses dir>: every network's license, as
# licenses/<name>-LICENSE.txt. Every network shipped needs one.
copy_model_licenses() {
    local model license
    for model in "$1"/models/*.bin "$1"/models/*.safetensors; do
        [[ -f ${model%.*}.LICENSE ]] || fail "$(basename "$model") has no models/$(basename "${model%.*}").LICENSE"
    done
    for license in "$1"/models/*.LICENSE; do
        cp "$license" "$2/$(model_license_name "$license")-LICENSE.txt"
    done
}

# common_notices <repo root> <tag> <program, e.g. flex.exe>: the notices both packages share, for
# THIRD-PARTY-NOTICES.txt.
common_notices() {
    local root=$1 tag=$2 program=$3 jassub_version
    jassub_version=$(sed -n 's/^JASSUB \([0-9.]*\).*/\1/p' "$root/crates/flex-web/web/vendor/jassub/README.txt")
    cat <<EOF
NeuFlow v2 (models/flow.safetensors; finds how the picture moves for video effects)
  License: Apache 2.0 (licenses/neuflow-LICENSE.txt).
  Source: https://github.com/neufieldrobotics/NeuFlow_v2

YuNet (models/yunet.bin; finds faces for video effects)
  License: MIT (licenses/yunet-LICENSE.txt).
  Source: https://github.com/opencv/opencv_zoo/tree/main/models/face_detection_yunet

MediaPipe Face Mesh V2 (models/face_mesh.bin; a detailed mesh of a face for video effects)
  License: Apache 2.0 (licenses/face-mesh-LICENSE.txt).
  Source: https://ai.google.dev/edge/mediapipe/solutions/vision/face_landmarker

Head pose network (models/head_angle.bin; which way heads point for video effects)
  License: MIT (licenses/head-angle-LICENSE.txt).
  Source: https://github.com/yakhyo/head-pose-estimation

YAMNet (models/sound.safetensors; what kind of sound is playing, for video effects)
  License: Apache 2.0 (licenses/yamnet-LICENSE.txt).
  Source: https://github.com/tensorflow/models/tree/master/research/audioset/yamnet

Flex's scene and body network (models/moge-segment.safetensors; runs on MoGe-2 ViT-S's
features, below, for video effects: people, their parts, walls, sky and more)
  Flex's own, trained on the labels of COCO panoptic and DensePose-COCO (cocodataset.org),
  ADE20K's Scene Parsing set (ade20k.csail.mit.edu), ATR, CIHP, Danbooru-Parsing and skytnt's
  anime-segmentation.
  License: MIT, as Flex (licenses/moge-segment-LICENSE.txt).
  Source: https://github.com/catchouli/flex/tree/$tag/crates/flex-depth/tools/moge/segment

MoGe-2 ViT-S by Microsoft (how far away everything is and which way it faces, for effects that
use depth, people, walls and sky)
  Not included: downloaded from Hugging Face the first time an effect needs it.
  License: MIT.
  Source: https://github.com/microsoft/MoGe and https://huggingface.co/Ruicheng/moge-2-vits-normal

AnimeGANv3 by Asher Chan (redraws the picture as anime for the Anime, Drawn World and Drawn
People effects)
  Not included: downloaded from the author's own releases the first time a style is used.
  License: free for non-commercial use only; commercial use needs the author's permission.
  Source: https://github.com/TachibanaYoshino/AnimeGANv3 and
  https://github.com/TachibanaYoshino/AnimeGANv3_Portrait_Inference (Ghibli and sketch styles)

Fast neural style (repaints the picture in a painting's style for the Painting, Drawn World and
Drawn People effects)
  Not included: downloaded from the ONNX Model Zoo the first time a style is used.
  License: BSD 3-Clause (the ONNX Model Zoo's; trained with PyTorch's examples).
  Source: https://huggingface.co/onnxmodelzoo (mosaic-9, candy-9, rain-princess-9, udnie-9,
  pointilism-9) and https://github.com/pytorch/examples/tree/main/fast_neural_style

Informative Drawings by Caroline Chan, Fredo Durand and Phillip Isola (line drawings for the
Line Art, Drawn World and Drawn People effects)
  Not included: downloaded the first time a style is used, from the copy of its weights in
  ControlNet's annotators (https://huggingface.co/lllyasviel/Annotators: sk_model.pth and
  sk_model2.pth).
  License: MIT.
  Source: https://github.com/carolineec/informative-drawings

Magenta's arbitrary image stylization by Golnaz Ghiasi, Honglak Lee, Manjunath Kudlur, Vincent
Dumoulin and Jonathon Shlens (paints in the style of any picture for the Paint Like, Drawn World
and Drawn People effects)
  Not included: downloaded from Kaggle Models the first time a painting style is used.
  License: Apache 2.0.
  Source: https://www.kaggle.com/models/google/arbitrary-image-stylization-v1 (TensorFlow 1,
  256, version 2) and https://github.com/magenta/magenta
  The paintings whose styles Flex comes with (their styles only, not the pictures) are all in
  the public domain, from Wikimedia Commons.

Self-Organising Textures by Eyvind Niklasson, Alexander Mordvintsev, Ettore Randazzo and Michael
Levin (the living textures grown for the Living Textures, Drawn World and Drawn People effects)
  Not included: the article's trained models are downloaded the first time a texture is used.
  License: CC BY 4.0 (the article and its models; its demo's code, which Flex's follows, is
  Apache 2.0).
  Source: https://distill.pub/selforg/2021/textures and
  https://github.com/distillpub/post--selforg-textures

DeepFilterNet3 by Hendrik Schröter and others (finds the voice in the sound, for sound effects)
  Not included: downloaded from its GitHub repository the first time a sound effect is on.
  License: MIT or Apache 2.0.
  Source: https://github.com/Rikorose/DeepFilterNet

Silero VAD (hears when someone's talking, for sound effects)
  Not included: downloaded from its GitHub repository the first time a sound effect is on.
  License: MIT.
  Source: https://github.com/snakers4/silero-vad

DDSP-VST by Google's Magenta team (hears a voice's pitch for the Instrument, Auto-tune, Synth
voice and some Voice changer voices, and plays it on instruments)
  Not included: the pitch network is downloaded from GitHub, and the instruments from Google's
  release, the first time they're used.
  License: Apache 2.0.
  Source: https://github.com/magenta/ddsp-vst

JASSUB $jassub_version (built into $program; draws subtitles in the web UI's player)
  JASSUB itself: MIT (licenses/jassub-LICENSE.txt). Its WebAssembly contains libass
  (ISC), FreeType (FTL or GPL 2), HarfBuzz (MIT), FriBidi (LGPL 2.1 or later) and their
  dependencies; Liberation Sans (SIL Open Font License) is its default font.
  Source, including those libraries: https://github.com/ThaUnknown/jassub/tree/v$jassub_version

Rust crates compiled into $program and the plugins: licenses/rust-crates.txt
EOF
}
