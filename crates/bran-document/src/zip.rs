//! Bounded ZIP reader and deterministic writer (stored and deflate only).

use crate::{Cancel, Limits, Refusal};

/// One inflated ZIP entry, in central-directory order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub name: String,
    pub data: Vec<u8>,
}

/// One entry to write. `dos_time`/`dos_date` are the MS-DOS timestamp fields.
#[derive(Clone, Copy, Debug)]
pub struct WriteEntry<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
    pub deflate: bool,
    pub dos_time: u16,
    pub dos_date: u16,
}

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const END: u32 = 0x0605_4b50;
const ZIP64_LOCATOR: u32 = 0x0706_4b50;
const COMPOUND_FILE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, Refusal> {
    let field = bytes.get(at..at.checked_add(2).ok_or(Refusal::MalformedContainer)?);
    let field = field.ok_or(Refusal::MalformedContainer)?;
    Ok(u16::from_le_bytes([field[0], field[1]]))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, Refusal> {
    let field = bytes.get(at..at.checked_add(4).ok_or(Refusal::MalformedContainer)?);
    let field = field.ok_or(Refusal::MalformedContainer)?;
    Ok(u32::from_le_bytes([field[0], field[1], field[2], field[3]]))
}

fn slice(bytes: &[u8], at: usize, len: usize) -> Result<&[u8], Refusal> {
    let end = at.checked_add(len).ok_or(Refusal::MalformedContainer)?;
    bytes.get(at..end).ok_or(Refusal::MalformedContainer)
}

struct Central {
    name: String,
    flags: u16,
    method: u16,
    crc: u32,
    compressed: usize,
    size: u64,
    local: usize,
}

/// Reads every entry, refusing anything outside stored/deflate single-disk
/// ZIP. Inflation stops one byte past the smallest applicable budget, so a
/// hostile entry costs at most that much work.
pub fn read(bytes: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Vec<Entry>, Refusal> {
    cancel.check()?;
    if bytes.len() as u64 > limits.max_package_bytes {
        return Err(Refusal::Oversized);
    }
    if bytes.starts_with(&COMPOUND_FILE) {
        return Err(Refusal::Encrypted);
    }
    // The end record is the last 22 bytes plus a comment of declared length.
    let end = (0..bytes.len().saturating_sub(21))
        .rev()
        .take(22 + 0xFFFF)
        .find(|&at| {
            u32_at(bytes, at) == Ok(END)
                && u16_at(bytes, at + 20).map(|len| at + 22 + len as usize) == Ok(bytes.len())
        })
        .ok_or(Refusal::MalformedContainer)?;
    if end >= 20 && u32_at(bytes, end - 20) == Ok(ZIP64_LOCATOR) {
        return Err(Refusal::UnsupportedContainer);
    }
    let (disk, cd_disk) = (u16_at(bytes, end + 4)?, u16_at(bytes, end + 6)?);
    let (here, total) = (u16_at(bytes, end + 8)?, u16_at(bytes, end + 10)?);
    let (cd_size, cd_offset) = (u32_at(bytes, end + 12)?, u32_at(bytes, end + 16)?);
    if total == 0xFFFF || cd_size == u32::MAX || cd_offset == u32::MAX {
        return Err(Refusal::UnsupportedContainer);
    }
    if disk != 0 || cd_disk != 0 || here != total {
        return Err(Refusal::UnsupportedContainer);
    }
    if total as usize > limits.max_parts {
        return Err(Refusal::TooManyParts);
    }
    let (cd_offset, cd_end) = (cd_offset as usize, cd_offset as usize + cd_size as usize);
    if cd_end != end {
        return Err(Refusal::MalformedContainer);
    }
    let mut records = Vec::with_capacity(total as usize);
    let mut at = cd_offset;
    for _ in 0..total {
        if u32_at(bytes, at)? != CENTRAL {
            return Err(Refusal::MalformedContainer);
        }
        let name_len = u16_at(bytes, at + 28)? as usize;
        let extra_len = u16_at(bytes, at + 30)? as usize;
        let comment_len = u16_at(bytes, at + 32)? as usize;
        let name = slice(bytes, at + 46, name_len)?;
        let name = String::from_utf8(name.to_vec()).map_err(|_| Refusal::UnsafePartPath)?;
        let (compressed, size, local) = (
            u32_at(bytes, at + 20)?,
            u32_at(bytes, at + 24)?,
            u32_at(bytes, at + 42)?,
        );
        if compressed == u32::MAX
            || size == u32::MAX
            || local == u32::MAX
            || u16_at(bytes, at + 34)? != 0
        {
            return Err(Refusal::UnsupportedContainer);
        }
        records.push(Central {
            name,
            flags: u16_at(bytes, at + 8)?,
            method: u16_at(bytes, at + 10)?,
            crc: u32_at(bytes, at + 16)?,
            compressed: compressed as usize,
            size: size as u64,
            local: local as usize,
        });
        at += 46 + name_len + extra_len + comment_len;
    }
    if at != cd_end {
        return Err(Refusal::MalformedContainer);
    }
    let mut names = std::collections::BTreeSet::new();
    let mut spans = Vec::with_capacity(records.len());
    for record in &records {
        if !names.insert(record.name.as_str()) {
            return Err(Refusal::DuplicatePart);
        }
        if record.flags & 0x0041 != 0 {
            return Err(Refusal::Encrypted);
        }
        if record.method != 0 && record.method != 8 {
            return Err(Refusal::UnsupportedContainer);
        }
        if u32_at(bytes, record.local)? != LOCAL {
            return Err(Refusal::MalformedContainer);
        }
        let name_len = u16_at(bytes, record.local + 26)? as usize;
        let extra_len = u16_at(bytes, record.local + 28)? as usize;
        if slice(bytes, record.local + 30, name_len)? != record.name.as_bytes() {
            return Err(Refusal::MalformedContainer);
        }
        let data = record.local + 30 + name_len + extra_len;
        let data_end = data
            .checked_add(record.compressed)
            .ok_or(Refusal::MalformedContainer)?;
        if data_end > cd_offset {
            return Err(Refusal::MalformedContainer);
        }
        spans.push((record.local, data_end));
    }
    spans.sort_unstable();
    if spans.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(Refusal::MalformedContainer);
    }
    let mut remaining = limits.max_total_bytes;
    let mut entries = Vec::with_capacity(records.len());
    for record in records {
        cancel.check()?;
        let name_len = u16_at(bytes, record.local + 26)? as usize;
        let extra_len = u16_at(bytes, record.local + 28)? as usize;
        let raw = slice(
            bytes,
            record.local + 30 + name_len + extra_len,
            record.compressed,
        )?;
        let budget = limits.max_part_bytes.min(remaining);
        let data = if record.method == 0 {
            if record.size != raw.len() as u64 {
                return Err(Refusal::MalformedContainer);
            }
            if record.size > budget {
                return Err(Refusal::Oversized);
            }
            raw.to_vec()
        } else {
            if record.size > budget
                || record.size > limits.max_ratio.saturating_mul(raw.len().max(1) as u64)
            {
                return Err(Refusal::DecompressionLimit);
            }
            inflate(raw, record.size)?
        };
        if crc32fast::hash(&data) != record.crc {
            return Err(Refusal::MalformedContainer);
        }
        remaining -= data.len() as u64;
        entries.push(Entry {
            name: record.name,
            data,
        });
    }
    Ok(entries)
}

/// Inflates at most `size + 1` bytes; any other length is a size lie.
fn inflate(raw: impl std::io::Read, size: u64) -> Result<Vec<u8>, Refusal> {
    use std::io::Read;
    let mut data = Vec::with_capacity(size as usize);
    flate2::read::DeflateDecoder::new(raw)
        .take(size + 1)
        .read_to_end(&mut data)
        .map_err(|_| Refusal::MalformedContainer)?;
    if data.len() as u64 != size {
        return Err(Refusal::MalformedContainer);
    }
    Ok(data)
}

/// Writes entries in the given order with exactly the given names and
/// timestamps. Export callers pass sorted names and one fixed timestamp.
pub fn write(entries: &[WriteEntry<'_>]) -> Vec<u8> {
    use std::io::Write;
    let mut out = Vec::new();
    let mut central = Vec::new();
    for entry in entries {
        let data = if entry.deflate {
            let mut encoder =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(entry.data).expect("in-memory write");
            encoder.finish().expect("in-memory write")
        } else {
            entry.data.to_vec()
        };
        let method: u16 = if entry.deflate { 8 } else { 0 };
        let crc = crc32fast::hash(entry.data);
        let local = out.len() as u32;
        let name = entry.name.as_bytes();
        let fields = |record: &mut Vec<u8>| {
            record.extend_from_slice(&0x0800u16.to_le_bytes()); // flags: UTF-8 names
            record.extend_from_slice(&method.to_le_bytes());
            record.extend_from_slice(&entry.dos_time.to_le_bytes());
            record.extend_from_slice(&entry.dos_date.to_le_bytes());
            record.extend_from_slice(&crc.to_le_bytes());
            record.extend_from_slice(&(data.len() as u32).to_le_bytes());
            record.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
            record.extend_from_slice(&(name.len() as u16).to_le_bytes());
            record.extend_from_slice(&0u16.to_le_bytes()); // extra length
        };
        out.extend_from_slice(&LOCAL.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        fields(&mut out);
        out.extend_from_slice(name);
        out.extend_from_slice(&data);
        central.extend_from_slice(&CENTRAL.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        fields(&mut central);
        central.extend_from_slice(&[0; 6]); // comment length, disk, internal attributes
        central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        central.extend_from_slice(&local.to_le_bytes());
        central.extend_from_slice(name);
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&END.to_le_bytes());
    out.extend_from_slice(&[0; 4]); // disk numbers
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length
    out
}

#[cfg(test)]
mod tests {
    use super::inflate;
    use crate::Refusal;
    use std::io::{Read, Write};

    struct Counting<'a>(&'a [u8], usize);

    impl Read for Counting<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let read = self.0.read(buffer)?;
            self.1 += read;
            Ok(read)
        }
    }

    #[test]
    fn inflation_stops_one_byte_past_the_declared_size() {
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        let noise: Vec<u8> = (0..1024 * 1024)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&noise).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut counting = Counting(&compressed, 0);
        assert_eq!(
            inflate(&mut counting, 1000),
            Err(Refusal::MalformedContainer)
        );
        assert!(
            counting.1 < compressed.len() / 4,
            "a size lie must stop inflation early: read {} of {} compressed bytes",
            counting.1,
            compressed.len()
        );
    }
}
