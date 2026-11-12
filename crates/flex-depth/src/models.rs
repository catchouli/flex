//! The networks' weights, which ship as files in a `models` folder next to Flex's program rather
//! than inside it. A file is only used if it's the one Flex was built with (its size and SHA-256
//! are noted at build time), so a missing or mismatched file turns off what needs it, with a
//! message, and nothing else.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

include!(concat!(env!("OUT_DIR"), "/models.rs"));

/// Where models are looked for: `models` next to Flex's program (where a release has them), then
/// the source folder, so `cargo run` and tests work without copying anything.
pub fn dirs() -> Vec<PathBuf> {
    let beside = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|d| d.join("models")));
    let source = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../models"));
    beside.into_iter().chain([source]).collect()
}

/// The model file `name` (as in the `models` folder), checked.
pub fn load(name: &str) -> Result<Vec<u8>> {
    let &(_, size, hash) = KNOWN.iter().find(|k| k.0 == name).with_context(|| format!("{name} isn't a model Flex knows"))?;
    let mut problem = None;
    for path in dirs().into_iter().map(|d| d.join(name)) {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let found: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        if bytes.len() as u64 == size && found == hash {
            return Ok(bytes);
        }
        problem = Some(format!("{} isn't the one this Flex was built with", path.display()));
    }
    match problem {
        Some(problem) => bail!(problem),
        None => bail!("{name} is missing (it belongs in a models folder next to Flex)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_loads() {
        assert!(!KNOWN.is_empty());
        for (name, size, _) in KNOWN {
            assert_eq!(load(name).unwrap().len() as u64, *size);
        }
        assert!(load("nothing.bin").is_err());
    }
}
