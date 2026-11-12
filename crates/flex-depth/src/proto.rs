//! Just enough of protobuf's wire format to read an ONNX file's graph (`animegan`) and a
//! TensorFlow SavedModel's (`saved_model`).

use anyhow::{Context, Result, bail, ensure};

pub(crate) struct Proto<'a> {
    pub(crate) b: &'a [u8],
}

pub(crate) enum Field<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed,
}

impl<'a> Proto<'a> {
    pub(crate) fn varint(&mut self) -> Result<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let (&byte, rest) = self.b.split_first().context("protobuf: cut short")?;
            self.b = rest;
            v |= u64::from(byte & 0x7f) << shift;
            if byte < 0x80 {
                return Ok(v);
            }
        }
        bail!("protobuf: a number too long")
    }

    pub(crate) fn next(&mut self) -> Result<Option<(u64, Field<'a>)>> {
        if self.b.is_empty() {
            return Ok(None);
        }
        let key = self.varint()?;
        let field = match key & 7 {
            0 => Field::Varint(self.varint()?),
            1 | 5 => {
                let len = if key & 7 == 1 { 8 } else { 4 };
                ensure!(self.b.len() >= len, "protobuf: cut short");
                self.b = &self.b[len..];
                Field::Fixed
            }
            2 => {
                let len = self.varint()? as usize;
                ensure!(self.b.len() >= len, "protobuf: cut short");
                let (bytes, rest) = self.b.split_at(len);
                self.b = rest;
                Field::Bytes(bytes)
            }
            other => bail!("protobuf: unexpected wire type {other}"),
        };
        Ok(Some((key >> 3, field)))
    }

    /// Each field `number` that holds bytes.
    pub(crate) fn all(b: &'a [u8], number: u64) -> Result<Vec<&'a [u8]>> {
        let mut out = Vec::new();
        let mut p = Proto { b };
        while let Some((n, f)) = p.next()? {
            if let (true, Field::Bytes(bytes)) = (n == number, f) {
                out.push(bytes);
            }
        }
        Ok(out)
    }
}

/// A packed repeated field's numbers.
pub(crate) fn packed(b: &[u8]) -> Result<Vec<u64>> {
    let mut p = Proto { b };
    let mut out = Vec::new();
    while !p.b.is_empty() {
        out.push(p.varint()?);
    }
    Ok(out)
}

/// Each varint field `number` in a message.
pub(crate) fn varints(b: &[u8], number: u64) -> Result<Vec<u64>> {
    let mut out = Vec::new();
    let mut p = Proto { b };
    while let Some((n, f)) = p.next()? {
        if let (true, Field::Varint(v)) = (n == number, f) {
            out.push(v);
        }
    }
    Ok(out)
}

/// The last varint field `number` in a message (as protobuf takes it), or 0.
pub(crate) fn varint(b: &[u8], number: u64) -> Result<u64> {
    Ok(varints(b, number)?.last().copied().unwrap_or(0))
}
