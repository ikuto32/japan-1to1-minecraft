//! Anvil region (.mca) reading and writing.

use anyhow::{Result, bail};
use std::io::{Read, Write};
use std::path::Path;

/// Read one chunk's uncompressed NBT bytes from a region file. `lx`,`lz` in 0..32.
pub fn read_chunk(path: &Path, lx: usize, lz: usize) -> Result<Option<Vec<u8>>> {
    let d = std::fs::read(path)?;
    let i = (lx + lz * 32) * 4;
    let off = ((d[i] as usize) << 16 | (d[i + 1] as usize) << 8 | d[i + 2] as usize) * 4096;
    if off == 0 {
        return Ok(None);
    }
    let len = u32::from_be_bytes(d[off..off + 4].try_into()?) as usize;
    let comp = d[off + 4];
    let body = &d[off + 5..off + 4 + len];
    let mut out = Vec::new();
    match comp {
        1 => {
            flate2::read::GzDecoder::new(body).read_to_end(&mut out)?;
        }
        2 => {
            flate2::read::ZlibDecoder::new(body).read_to_end(&mut out)?;
        }
        3 => out.extend_from_slice(body),
        c => bail!("unsupported chunk compression {c}"),
    }
    Ok(Some(out))
}

/// Compress chunk NBT with zlib.
pub fn compress_chunk(nbt: &[u8], level: u32) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::with_capacity(nbt.len() / 8 + 64), flate2::Compression::new(level));
    e.write_all(nbt).unwrap();
    e.finish().unwrap()
}

/// Assemble a region file from up to 1024 zlib-compressed chunk payloads,
/// indexed by lx + lz*32.
pub fn build_region(chunks: &[Option<Vec<u8>>], timestamp: u32) -> Result<Vec<u8>> {
    let mut out = vec![0u8; 8192];
    for (i, c) in chunks.iter().enumerate() {
        let Some(c) = c else { continue };
        let total = c.len() + 5;
        let sectors = total.div_ceil(4096);
        if sectors > 255 {
            bail!("chunk too large ({} bytes)", total);
        }
        let off = out.len() / 4096;
        out[i * 4] = (off >> 16) as u8;
        out[i * 4 + 1] = (off >> 8) as u8;
        out[i * 4 + 2] = off as u8;
        out[i * 4 + 3] = sectors as u8;
        out[4096 + i * 4..4096 + i * 4 + 4].copy_from_slice(&timestamp.to_be_bytes());
        out.extend_from_slice(&((c.len() + 1) as u32).to_be_bytes());
        out.push(2);
        out.extend_from_slice(c);
        out.resize(out.len().div_ceil(4096) * 4096, 0);
    }
    Ok(out)
}
