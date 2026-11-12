//! The tensors in a file saved by `torch.save` (a zip of a pickle and raw storages), read without
//! Python: only as much of the pickle format as a saved dictionary of float tensors uses, and
//! nothing in it is run.

use std::collections::HashMap;

use anyhow::{Context, Result, bail, ensure};

use crate::Tensors;

/// The stored (uncompressed) files in a zip: name → contents.
fn zip_entries(bytes: &[u8]) -> Result<HashMap<String, &[u8]>> {
    let u16_at = |at: usize| -> Result<usize> { Ok(u16::from_le_bytes(bytes.get(at..at + 2).context("zip: truncated")?.try_into()?) as usize) };
    let u32_at = |at: usize| -> Result<usize> { Ok(u32::from_le_bytes(bytes.get(at..at + 4).context("zip: truncated")?.try_into()?) as usize) };
    // The end of central directory record, within the last 64 KB (its comment can be that long).
    let end = (bytes.len().saturating_sub(22 + 65535)..bytes.len().saturating_sub(21))
        .rev()
        .find(|&i| bytes[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        .context("not a zip file")?;
    let (count, mut at) = (u16_at(end + 10)?, u32_at(end + 16)?);
    let mut out = HashMap::new();
    for _ in 0..count {
        if u32_at(at)? != 0x0201_4b50 {
            bail!("zip: bad central directory");
        }
        let (method, size) = (u16_at(at + 10)?, u32_at(at + 20)?);
        let (name_len, extra_len, comment_len) = (u16_at(at + 28)?, u16_at(at + 30)?, u16_at(at + 32)?);
        let local = u32_at(at + 42)?;
        let name = String::from_utf8_lossy(bytes.get(at + 46..at + 46 + name_len).context("zip: truncated")?).into_owned();
        at += 46 + name_len + extra_len + comment_len;
        if method != 0 {
            bail!("zip: {name} is compressed");
        }
        if u32_at(local)? != 0x0403_4b50 {
            bail!("zip: bad local header for {name}");
        }
        let data = local + 30 + u16_at(local + 26)? + u16_at(local + 28)?;
        out.insert(name, bytes.get(data..data + size).context("zip: data out of range")?);
    }
    Ok(out)
}

#[derive(Clone, Debug)]
enum Value {
    None,
    Bool,
    Int(i64),
    Str(String),
    List(Vec<Value>),
    Tuple(Vec<Value>),
    Dict(Vec<(Value, Value)>),
    Global(String),
    /// A storage's file in the zip.
    Storage(String),
    Tensor { storage: String, offset: usize, size: Vec<usize>, stride: Vec<usize> },
    Mark,
}

fn ints(v: &Value) -> Result<Vec<usize>> {
    match v {
        Value::Tuple(items) => items.iter().map(|i| if let Value::Int(n) = i { usize::try_from(*n).context("pickle: a negative size") } else { bail!("pickle: expected a number") }).collect(),
        _ => bail!("pickle: expected a tuple of numbers"),
    }
}

/// Runs the pickle `data` as far as building the values it describes.
fn unpickle(data: &[u8]) -> Result<Value> {
    let mut stack: Vec<Value> = Vec::new();
    let mut memo: HashMap<usize, Value> = HashMap::new();
    let mut i = 0;
    let take = |i: &mut usize, n: usize| -> Result<&[u8]> {
        let s = data.get(*i..*i + n).context("pickle: truncated")?;
        *i += n;
        Ok(s)
    };
    let pop = |stack: &mut Vec<Value>| stack.pop().context("pickle: empty stack");
    let since_mark = |stack: &mut Vec<Value>| -> Result<Vec<Value>> {
        let at = stack.iter().rposition(|v| matches!(v, Value::Mark)).context("pickle: no mark")?;
        let items = stack.split_off(at + 1);
        stack.pop();
        Ok(items)
    };
    loop {
        let op = *data.get(i).context("pickle: no STOP")?;
        i += 1;
        match op {
            0x80 => i += 1, // PROTO
            b'.' => return pop(&mut stack),
            b'}' => stack.push(Value::Dict(Vec::new())),
            b']' => stack.push(Value::List(Vec::new())),
            b')' => stack.push(Value::Tuple(Vec::new())),
            b'N' => stack.push(Value::None),
            0x88 | 0x89 => stack.push(Value::Bool),
            b'(' => stack.push(Value::Mark),
            b'K' => stack.push(Value::Int(i64::from(take(&mut i, 1)?[0]))),
            b'M' => stack.push(Value::Int(i64::from(u16::from_le_bytes(take(&mut i, 2)?.try_into()?)))),
            b'J' => stack.push(Value::Int(i64::from(i32::from_le_bytes(take(&mut i, 4)?.try_into()?)))),
            b'X' => {
                let n = u32::from_le_bytes(take(&mut i, 4)?.try_into()?) as usize;
                stack.push(Value::Str(String::from_utf8_lossy(take(&mut i, n)?).into_owned()));
            }
            b'c' => {
                let rest = &data[i..];
                let a = rest.iter().position(|b| *b == b'\n').context("pickle: bad GLOBAL")?;
                let b = rest[a + 1..].iter().position(|b| *b == b'\n').context("pickle: bad GLOBAL")?;
                let name = format!("{} {}", String::from_utf8_lossy(&rest[..a]), String::from_utf8_lossy(&rest[a + 1..a + 1 + b]));
                i += a + b + 2;
                stack.push(Value::Global(name));
            }
            b'q' => {
                let k = usize::from(take(&mut i, 1)?[0]);
                memo.insert(k, stack.last().context("pickle: empty stack")?.clone());
            }
            b'r' => {
                let k = u32::from_le_bytes(take(&mut i, 4)?.try_into()?) as usize;
                memo.insert(k, stack.last().context("pickle: empty stack")?.clone());
            }
            b'h' => {
                let k = usize::from(take(&mut i, 1)?[0]);
                stack.push(memo.get(&k).context("pickle: bad memo")?.clone());
            }
            b'j' => {
                let k = u32::from_le_bytes(take(&mut i, 4)?.try_into()?) as usize;
                stack.push(memo.get(&k).context("pickle: bad memo")?.clone());
            }
            b't' => {
                let items = since_mark(&mut stack)?;
                stack.push(Value::Tuple(items));
            }
            0x85..=0x87 => {
                let n = usize::from(op - 0x84);
                let items = stack.split_off(stack.len().checked_sub(n).context("pickle: short stack")?);
                stack.push(Value::Tuple(items));
            }
            b'a' => {
                let v = pop(&mut stack)?;
                if let Some(Value::List(l)) = stack.last_mut() {
                    l.push(v);
                }
            }
            b'e' => {
                let items = since_mark(&mut stack)?;
                if let Some(Value::List(l)) = stack.last_mut() {
                    l.extend(items);
                }
            }
            b's' => {
                let v = pop(&mut stack)?;
                let k = pop(&mut stack)?;
                if let Some(Value::Dict(d)) = stack.last_mut() {
                    d.push((k, v));
                }
            }
            b'u' => {
                let items = since_mark(&mut stack)?;
                if let Some(Value::Dict(d)) = stack.last_mut() {
                    let mut it = items.into_iter();
                    while let (Some(k), Some(v)) = (it.next(), it.next()) {
                        d.push((k, v));
                    }
                }
            }
            b'Q' => {
                // ('storage', type, key, location, size)
                let Value::Tuple(id) = pop(&mut stack)? else { bail!("pickle: bad persistent id") };
                let Some(Value::Str(key)) = id.get(2) else { bail!("pickle: bad persistent id") };
                if !matches!(id.get(1), Some(Value::Global(g)) if g == "torch FloatStorage") {
                    bail!("pickle: only float tensors are supported");
                }
                stack.push(Value::Storage(key.clone()));
            }
            b'R' => {
                let args = pop(&mut stack)?;
                let Value::Global(f) = pop(&mut stack)? else { bail!("pickle: REDUCE of something not a function") };
                let Value::Tuple(args) = args else { bail!("pickle: bad REDUCE arguments") };
                stack.push(match f.as_str() {
                    "collections OrderedDict" => Value::Dict(Vec::new()),
                    "torch._utils _rebuild_tensor_v2" => {
                        let (Some(Value::Storage(storage)), Some(Value::Int(offset)), Some(size), Some(stride)) = (args.first(), args.get(1), args.get(2), args.get(3)) else {
                            bail!("pickle: bad tensor")
                        };
                        Value::Tensor { storage: storage.clone(), offset: usize::try_from(*offset).context("pickle: a negative offset")?, size: ints(size)?, stride: ints(stride)? }
                    }
                    other => bail!("pickle: won't call {other}"),
                });
            }
            b'b' => {
                // BUILD: an OrderedDict's state (its metadata), which isn't needed.
                pop(&mut stack)?;
            }
            other => bail!("pickle: unsupported opcode {other:#04x}"),
        }
    }
}

/// The float tensors under `key` (for example "model") in the file's top dictionary, or with
/// `None`, the top dictionary's own (a saved `state_dict`).
pub fn read(bytes: &[u8], key: Option<&str>) -> Result<Tensors> {
    let files = zip_entries(bytes)?;
    let (pickle_name, pickle) = files.iter().find(|(n, _)| n.ends_with("/data.pkl")).context("no data.pkl in the file")?;
    let root = pickle_name.trim_end_matches("data.pkl");
    if files.get(&format!("{root}byteorder")).is_some_and(|b| *b != b"little") {
        bail!("big-endian tensors aren't supported");
    }
    let Value::Dict(top) = unpickle(pickle)? else { bail!("the file isn't a dictionary") };
    let tensors = match key {
        None => &top,
        Some(key) => match top.iter().find(|(k, _)| matches!(k, Value::Str(s) if s == key)) {
            Some((_, Value::Dict(tensors))) => tensors,
            _ => bail!("no {key} in the file"),
        },
    };
    let mut out = HashMap::new();
    for (name, t) in tensors {
        let (Value::Str(name), Value::Tensor { storage, offset, size, stride }) = (name, t) else { continue };
        let raw = files.get(&format!("{root}data/{storage}")).with_context(|| format!("{name}: no storage {storage}"))?;
        let floats: Vec<f32> = raw.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        let count = size.iter().try_fold(1usize, |n, s| n.checked_mul(*s)).with_context(|| format!("{name}: too big"))?;
        ensure!(stride.len() == size.len(), "{name}: {} strides for {} sizes", stride.len(), size.len());
        // (More values than the storage holds only with a stride of 0: room for what it holds.)
        let mut values = Vec::with_capacity(count.min(floats.len()));
        for n in 0..count {
            let (mut rest, mut at) = (n, Some(*offset));
            for (s, st) in size.iter().zip(stride).rev() {
                at = at.and_then(|at| at.checked_add((rest % s).checked_mul(*st)?));
                rest /= s;
            }
            values.push(*at.and_then(|at| floats.get(at)).with_context(|| format!("{name}: out of its storage"))?);
        }
        out.insert(name.clone(), (size.clone(), values));
    }
    Ok(out)
}
