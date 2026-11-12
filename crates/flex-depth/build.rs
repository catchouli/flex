//! Notes the size and SHA-256 of each file in the top-level `models` folder, so `models.rs` can
//! tell when a file next to Flex isn't the one it was built with.

use std::fmt::Write as _;
use std::path::Path;

use sha2::{Digest, Sha256};

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models");
    println!("cargo::rerun-if-changed={}", dir.display());
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("no models folder")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "bin" || e == "safetensors"))
        .collect();
    files.sort();
    let mut out = String::from("/// Each model file's name, size and SHA-256.\nconst KNOWN: &[(&str, u64, &str)] = &[\n");
    for path in files {
        let bytes = std::fs::read(&path).expect("can't read a model");
        let name = path.file_name().unwrap().to_string_lossy();
        let hash: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        writeln!(out, "    ({name:?}, {}, {hash:?}),", bytes.len()).unwrap();
    }
    out.push_str("];\n");
    std::fs::write(Path::new(&std::env::var("OUT_DIR").unwrap()).join("models.rs"), out).unwrap();
}
