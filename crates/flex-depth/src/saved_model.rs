//! The variables of a TensorFlow SavedModel, as Kaggle Models hands one out (a .tar.gz of
//! `saved_model.pb` and `variables/`), read without TensorFlow: the tar, the checkpoint's index
//! (a LevelDB table of where each tensor is in the data files) and, for the variables' names, the
//! SavedModel's object graph (the checkpoint only numbers them).

use std::collections::HashMap;
use std::io::Read;
use std::ops::Range;

use anyhow::{Context, Result, bail, ensure};

use crate::Tensors;
use crate::proto::{Proto, varint};

/// Where each file in a tar is: name (without any leading "./") → its bytes' range.
type Files = HashMap<String, Range<usize>>;

/// A .tar.gz's contents, and where each file is in them. (Not each file copied out: that would
/// hold everything twice.)
fn untar(gz: &[u8]) -> Result<(Vec<u8>, Files)> {
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(gz).read_to_end(&mut tar).context("not a .tar.gz")?;
    let mut out = HashMap::new();
    let mut at = 0;
    while at + 512 <= tar.len() {
        let header = &tar[at..at + 512];
        if header.iter().all(|&b| b == 0) {
            break;
        }
        let text = |r: Range<usize>| String::from_utf8_lossy(&header[r]).trim_end_matches('\0').to_string();
        let octal = text(124..136);
        let size = usize::from_str_radix(octal.trim_matches(|c: char| c == ' ' || c == '\0'), 8).with_context(|| format!("tar: bad size {octal:?}"))?;
        // ustar's prefix, for long names.
        let (prefix, name) = (text(345..500), text(0..100));
        let name = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let data = at + 512..(at + 512).checked_add(size).filter(|&end| end <= tar.len()).context("tar: cut short")?;
        // Ordinary files only ('0', or NUL from old tars).
        if matches!(header[156], b'0' | 0) {
            out.insert(name.trim_start_matches("./").to_string(), data);
        }
        at += 512 + size.div_ceil(512) * 512;
    }
    Ok((tar, out))
}

/// The checkpoint's keys for the SavedModel's variables ("variables/7/.ATTRIBUTES/VARIABLE_VALUE")
/// → their names ("transformer/contract/conv1/weights"), from the object graph: the root's
/// child "variables" is a list whose children ("0", "1", ...) are the variables.
fn variable_names(saved_model: &[u8]) -> Result<HashMap<String, String>> {
    let meta = *Proto::all(saved_model, 2)?.first().context("SavedModel: no meta graph")?;
    let objects = *Proto::all(meta, 7)?.first().context("SavedModel: no object graph")?;
    let nodes = Proto::all(objects, 1)?;
    // A node's children: local name → node.
    let children = |node: &[u8]| -> Result<Vec<(String, usize)>> {
        Proto::all(node, 1)?
            .into_iter()
            .map(|r| {
                let name = Proto::all(r, 2)?.first().map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
                Ok((name, varint(r, 1)? as usize))
            })
            .collect()
    };
    let root = nodes.first().context("SavedModel: an empty object graph")?;
    let list = children(root)?.into_iter().find(|(n, _)| n == "variables").context("SavedModel: no variables")?.1;
    let mut out = HashMap::new();
    for (local, id) in children(nodes.get(list).context("SavedModel: bad object graph")?)? {
        let node = nodes.get(id).context("SavedModel: bad object graph")?;
        let Some(variable) = Proto::all(node, 7)?.first().copied() else { continue };
        let name = Proto::all(variable, 6)?.first().map(|b| String::from_utf8_lossy(b).into_owned()).context("SavedModel: a variable with no name")?;
        out.insert(format!("variables/{local}/.ATTRIBUTES/VARIABLE_VALUE"), name.trim_end_matches(":0").to_string());
    }
    Ok(out)
}

/// A Snappy-compressed block, uncompressed (the raw format: its length, then literals and copies).
fn unsnappy(b: &[u8]) -> Result<Vec<u8>> {
    let mut p = Proto { b };
    let len = usize::try_from(p.varint()?).context("snappy: bad length")?;
    let mut b = p.b;
    // Room for no more than the input could make: at most 64 bytes from a copy's 3.
    let mut out = Vec::with_capacity(len.min(b.len().saturating_mul(22)));
    let take = |b: &mut &[u8], n: usize| -> Result<usize> {
        ensure!(b.len() >= n, "snappy: cut short");
        let v = b[..n].iter().rev().fold(0usize, |v, &x| v << 8 | x as usize);
        *b = &b[n..];
        Ok(v)
    };
    while let Some((&tag, rest)) = b.split_first() {
        b = rest;
        let (n, offset) = match tag & 3 {
            0 => {
                let n = match tag >> 2 {
                    small @ 0..60 => small as usize + 1,
                    big => take(&mut b, big as usize - 59)? + 1,
                };
                ensure!(b.len() >= n, "snappy: cut short");
                ensure!(out.len() + n <= len, "snappy: more than {len} bytes");
                out.extend_from_slice(&b[..n]);
                b = &b[n..];
                continue;
            }
            1 => (((tag >> 2) & 7) as usize + 4, ((tag as usize >> 5) << 8) | take(&mut b, 1)?),
            2 => ((tag >> 2) as usize + 1, take(&mut b, 2)?),
            _ => ((tag >> 2) as usize + 1, take(&mut b, 4)?),
        };
        ensure!(offset > 0 && offset <= out.len(), "snappy: bad copy");
        ensure!(out.len() + n <= len, "snappy: more than {len} bytes");
        for _ in 0..n {
            out.push(out[out.len() - offset]);
        }
    }
    ensure!(out.len() == len, "snappy: {} bytes, not {len}", out.len());
    Ok(out)
}

/// A LevelDB table's entries: key → value.
fn table(b: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    ensure!(b.len() >= 48 && b[b.len() - 8..] == 0xdb47_7524_8b80_fb57u64.to_le_bytes(), "not a TensorFlow checkpoint index");
    let mut footer = Proto { b: &b[b.len() - 48..] };
    let (_meta, _meta_len) = (footer.varint()?, footer.varint()?);
    let index = (footer.varint()? as usize, footer.varint()? as usize);
    // Each block is followed by its compression (none, or Snappy) and a checksum.
    let block = |(at, len): (usize, usize)| -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let raw = at.checked_add(len).and_then(|end| b.get(at..end.checked_add(5)?)).context("checkpoint index: block out of range")?;
        let data = match raw[len] {
            0 => raw[..len].to_vec(),
            1 => unsnappy(&raw[..len])?,
            other => bail!("checkpoint index: compression {other} isn't supported"),
        };
        let len = data.len().checked_sub(4).context("checkpoint index: bad block")?;
        let restarts = u32::from_le_bytes(data[len..].try_into()?) as usize;
        let end = len.checked_sub(4 * restarts).context("checkpoint index: bad block")?;
        let mut p = Proto { b: &data[..end] };
        let mut key: Vec<u8> = Vec::new();
        let mut out = Vec::new();
        while !p.b.is_empty() {
            let (shared, own, value) = (p.varint()? as usize, p.varint()? as usize, p.varint()? as usize);
            ensure!(shared <= key.len() && own.checked_add(value).is_some_and(|n| p.b.len() >= n), "checkpoint index: bad entry");
            key.truncate(shared);
            key.extend_from_slice(&p.b[..own]);
            out.push((key.clone(), p.b[own..own + value].to_vec()));
            p.b = &p.b[own + value..];
        }
        Ok(out)
    };
    let mut out = Vec::new();
    for (_, handle) in block(index)? {
        let mut h = Proto { b: &handle };
        out.extend(block((h.varint()? as usize, h.varint()? as usize))?);
    }
    Ok(out)
}

/// The float variables of a SavedModel packed as a .tar.gz: name → (shape, values).
pub(crate) fn read(gz: &[u8]) -> Result<Tensors> {
    let (tar, files) = untar(gz)?;
    let file = |name: &str| files.get(name).map(|r| &tar[r.clone()]).with_context(|| format!("SavedModel: no {name}"));
    let names = variable_names(file("saved_model.pb")?)?;
    let shard = |id: u64| files.iter().find(|(k, _)| k.starts_with(&format!("variables/variables.data-{id:05}-of-"))).map(|(_, r)| &tar[r.clone()]);
    let mut out = HashMap::new();
    for (key, entry) in table(file("variables/variables.index")?)? {
        let Some(name) = names.get(String::from_utf8_lossy(&key).as_ref()) else { continue };
        // BundleEntryProto: dtype 1 (float), shape 2, shard 3, offset 4, size 5.
        let entry = entry.as_slice();
        if varint(entry, 1)? != 1 {
            continue;
        }
        let shape = match Proto::all(entry, 2)?.first() {
            Some(s) => Proto::all(s, 2)?.into_iter().map(|d| Ok(varint(d, 1)? as usize)).collect::<Result<Vec<_>>>()?,
            None => Vec::new(),
        };
        let (offset, size) = (varint(entry, 4)? as usize, varint(entry, 5)? as usize);
        let data = shard(varint(entry, 3)?).with_context(|| format!("SavedModel: no data for {name}"))?;
        let raw = offset.checked_add(size).and_then(|end| data.get(offset..end)).with_context(|| format!("SavedModel: {name} out of range"))?;
        if Some(size) != shape.iter().try_fold(4usize, |n, d| n.checked_mul(*d)) {
            bail!("SavedModel: {name} is {size} bytes for {shape:?}");
        }
        out.insert(name.clone(), (shape, raw.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()));
    }
    Ok(out)
}
