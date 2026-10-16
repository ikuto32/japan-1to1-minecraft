//! Minimal NBT (Java Edition, big-endian) reader/writer.

use anyhow::{Result, bail};
use std::fmt::Write as _;

#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(Vec<Tag>),
    Compound(Vec<(String, Tag)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Tag {
    pub fn id(&self) -> u8 {
        match self {
            Tag::Byte(_) => 1,
            Tag::Short(_) => 2,
            Tag::Int(_) => 3,
            Tag::Long(_) => 4,
            Tag::Float(_) => 5,
            Tag::Double(_) => 6,
            Tag::ByteArray(_) => 7,
            Tag::String(_) => 8,
            Tag::List(_) => 9,
            Tag::Compound(_) => 10,
            Tag::IntArray(_) => 11,
            Tag::LongArray(_) => 12,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Tag> {
        match self {
            Tag::Compound(v) => v.iter().find(|(k, _)| k == key).map(|(_, t)| t),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Tag> {
        match self {
            Tag::Compound(v) => v.iter_mut().find(|(k, _)| k == key).map(|(_, t)| t),
            _ => None,
        }
    }

    /// Insert or replace a key in a compound.
    pub fn set(&mut self, key: &str, val: Tag) {
        if let Tag::Compound(v) = self {
            if let Some(e) = v.iter_mut().find(|(k, _)| k == key) {
                e.1 = val;
            } else {
                v.push((key.to_string(), val));
            }
        }
    }

    pub fn remove(&mut self, key: &str) {
        if let Tag::Compound(v) = self {
            v.retain(|(k, _)| k != key);
        }
    }
}

// ---------------------------------------------------------------- writing

/// Streaming NBT writer into a byte buffer (fast path for chunk generation).
pub struct W {
    pub buf: Vec<u8>,
}

impl W {
    pub fn new() -> Self {
        W { buf: Vec::with_capacity(1 << 16) }
    }
    #[inline]
    fn name(&mut self, n: &str) {
        self.buf.extend_from_slice(&(n.len() as u16).to_be_bytes());
        self.buf.extend_from_slice(n.as_bytes());
    }
    #[inline]
    fn head(&mut self, id: u8, n: &str) {
        self.buf.push(id);
        self.name(n);
    }
    pub fn begin_root(&mut self) {
        self.head(10, "");
    }
    pub fn begin_compound(&mut self, n: &str) {
        self.head(10, n);
    }
    pub fn end(&mut self) {
        self.buf.push(0);
    }
    pub fn byte(&mut self, n: &str, v: i8) {
        self.head(1, n);
        self.buf.push(v as u8);
    }
    pub fn int(&mut self, n: &str, v: i32) {
        self.head(3, n);
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    pub fn long(&mut self, n: &str, v: i64) {
        self.head(4, n);
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    pub fn string(&mut self, n: &str, v: &str) {
        self.head(8, n);
        self.name(v);
    }
    pub fn byte_array(&mut self, n: &str, v: &[u8]) {
        self.head(7, n);
        self.buf.extend_from_slice(&(v.len() as i32).to_be_bytes());
        self.buf.extend_from_slice(v);
    }
    pub fn long_array(&mut self, n: &str, v: &[i64]) {
        self.head(12, n);
        self.buf.extend_from_slice(&(v.len() as i32).to_be_bytes());
        for x in v {
            self.buf.extend_from_slice(&x.to_be_bytes());
        }
    }
    /// Begin a list; caller writes `len` payloads of type `elem_id` without headers.
    pub fn begin_list(&mut self, n: &str, elem_id: u8, len: usize) {
        self.head(9, n);
        self.buf.push(if len == 0 { 0 } else { elem_id });
        self.buf.extend_from_slice(&(len as i32).to_be_bytes());
    }
    /// Raw string payload (for lists of strings).
    pub fn str_payload(&mut self, v: &str) {
        self.name(v);
    }
}

pub fn write_payload(buf: &mut Vec<u8>, t: &Tag) {
    match t {
        Tag::Byte(v) => buf.push(*v as u8),
        Tag::Short(v) => buf.extend_from_slice(&v.to_be_bytes()),
        Tag::Int(v) => buf.extend_from_slice(&v.to_be_bytes()),
        Tag::Long(v) => buf.extend_from_slice(&v.to_be_bytes()),
        Tag::Float(v) => buf.extend_from_slice(&v.to_be_bytes()),
        Tag::Double(v) => buf.extend_from_slice(&v.to_be_bytes()),
        Tag::ByteArray(v) => {
            buf.extend_from_slice(&(v.len() as i32).to_be_bytes());
            buf.extend(v.iter().map(|x| *x as u8));
        }
        Tag::String(s) => {
            buf.extend_from_slice(&(s.len() as u16).to_be_bytes());
            buf.extend_from_slice(s.as_bytes());
        }
        Tag::List(v) => {
            let id = v.first().map(|t| t.id()).unwrap_or(0);
            buf.push(id);
            buf.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for e in v {
                write_payload(buf, e);
            }
        }
        Tag::Compound(v) => {
            for (k, e) in v {
                buf.push(e.id());
                buf.extend_from_slice(&(k.len() as u16).to_be_bytes());
                buf.extend_from_slice(k.as_bytes());
                write_payload(buf, e);
            }
            buf.push(0);
        }
        Tag::IntArray(v) => {
            buf.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for x in v {
                buf.extend_from_slice(&x.to_be_bytes());
            }
        }
        Tag::LongArray(v) => {
            buf.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for x in v {
                buf.extend_from_slice(&x.to_be_bytes());
            }
        }
    }
}

/// Serialize a named root tag.
pub fn write_root(name: &str, t: &Tag) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(t.id());
    buf.extend_from_slice(&(name.len() as u16).to_be_bytes());
    buf.extend_from_slice(name.as_bytes());
    write_payload(&mut buf, t);
    buf
}

// ---------------------------------------------------------------- reading

struct R<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.p + n > self.d.len() {
            bail!("NBT truncated");
        }
        let s = &self.d[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into()?))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into()?))
    }
    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.take(8)?.try_into()?))
    }
    fn string(&mut self) -> Result<String> {
        let n = self.i16()? as u16 as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
    fn payload(&mut self, id: u8) -> Result<Tag> {
        Ok(match id {
            1 => Tag::Byte(self.u8()? as i8),
            2 => Tag::Short(self.i16()?),
            3 => Tag::Int(self.i32()?),
            4 => Tag::Long(self.i64()?),
            5 => Tag::Float(f32::from_bits(self.i32()? as u32)),
            6 => Tag::Double(f64::from_bits(self.i64()? as u64)),
            7 => {
                let n = self.i32()? as usize;
                Tag::ByteArray(self.take(n)?.iter().map(|x| *x as i8).collect())
            }
            8 => Tag::String(self.string()?),
            9 => {
                let eid = self.u8()?;
                let n = self.i32()?.max(0) as usize;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(self.payload(eid)?);
                }
                Tag::List(v)
            }
            10 => {
                let mut v = Vec::new();
                loop {
                    let id = self.u8()?;
                    if id == 0 {
                        break;
                    }
                    let k = self.string()?;
                    v.push((k, self.payload(id)?));
                }
                Tag::Compound(v)
            }
            11 => {
                let n = self.i32()? as usize;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(self.i32()?);
                }
                Tag::IntArray(v)
            }
            12 => {
                let n = self.i32()? as usize;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(self.i64()?);
                }
                Tag::LongArray(v)
            }
            _ => bail!("bad NBT tag id {id}"),
        })
    }
}

/// Parse a named root tag. Returns (name, tag).
pub fn read_root(d: &[u8]) -> Result<(String, Tag)> {
    let mut r = R { d, p: 0 };
    let id = r.u8()?;
    let name = r.string()?;
    Ok((name, r.payload(id)?))
}

/// Human-readable dump (arrays abbreviated).
pub fn dump(t: &Tag, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    match t {
        Tag::Compound(v) => {
            out.push_str("{\n");
            for (k, e) in v {
                let _ = write!(out, "{pad}  {k}: ");
                dump(e, indent + 1, out);
            }
            let _ = writeln!(out, "{pad}}}");
        }
        Tag::List(v) => {
            if v.iter().all(|e| !matches!(e, Tag::Compound(_) | Tag::List(_))) && v.len() <= 16 {
                let _ = writeln!(out, "{:?}", v);
            } else {
                let _ = writeln!(out, "[ ({} items)", v.len());
                for (i, e) in v.iter().enumerate() {
                    if i >= 6 && i + 3 < v.len() {
                        if i == 6 {
                            let _ = writeln!(out, "{pad}  ...");
                        }
                        continue;
                    }
                    let _ = write!(out, "{pad}  [{i}] ");
                    dump(e, indent + 1, out);
                }
                let _ = writeln!(out, "{pad}]");
            }
        }
        Tag::ByteArray(v) => {
            let _ = writeln!(out, "ByteArray[{}] {:?}...", v.len(), &v[..v.len().min(8)]);
        }
        Tag::IntArray(v) => {
            let _ = writeln!(out, "IntArray{:?}", &v[..v.len().min(16)]);
        }
        Tag::LongArray(v) => {
            let _ = writeln!(out, "LongArray[{}] {:?}...", v.len(), &v[..v.len().min(4)]);
        }
        other => {
            let _ = writeln!(out, "{:?}", other);
        }
    }
}
