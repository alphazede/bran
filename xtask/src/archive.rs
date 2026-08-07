//! Deterministic archive writing.
//!
//! These writers are hand-rolled so every byte is under our control. The
//! properties match what `tools/ci/build-release.sh` used to produce with
//! python3's gzip/tarfile/zipfile. Byte identity with that output is not
//! required; determinism is: the same binary always yields the same archive
//! bytes, on every host.

use std::fs;
use std::io::{self, Read, Seek, Write};
use std::path::Path;

/// Writes `data` as member `member` inside a gzip-compressed USTAR archive.
///
/// Gzip: no filename, no extra fields, mtime 0. Tar: USTAR, single regular
/// file member with mode 0o755, uid 0, gid 0, mtime 0, empty uname/gname.
pub fn write_tar_gz(path: &Path, member: &str, data: &[u8]) -> io::Result<()> {
    write_tar_gz_to(io::BufWriter::new(fs::File::create(path)?), member, data).map(|_| ())
}

fn write_tar_gz_to<W: Write>(mut writer: W, member: &str, data: &[u8]) -> io::Result<W> {
    let encoder = flate2::GzBuilder::new()
        .mtime(0)
        .write(&mut writer, flate2::Compression::default());
    let mut encoder = io::BufWriter::new(encoder);
    write_ustar(&mut encoder, member, data)?;
    encoder.flush()?;
    let encoder = encoder.into_inner().map_err(io::Error::from)?;
    encoder.finish()?;
    Ok(writer)
}

const TAR_BLOCK_SIZE: usize = 512;

/// Writes a single-member USTAR archive: one 512-byte header, the file data
/// padded to the block boundary, and two zero blocks ending the archive.
fn write_ustar<W: Write>(writer: &mut W, member: &str, data: &[u8]) -> io::Result<()> {
    if member.len() > 100 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("tar member name too long: {member}"),
        ));
    }
    let mut header = [0u8; TAR_BLOCK_SIZE];
    header[..member.len()].copy_from_slice(member.as_bytes());
    write_octal(&mut header[100..108], 0o755)?; // mode
    write_octal(&mut header[108..116], 0)?; // uid
    write_octal(&mut header[116..124], 0)?; // gid
    write_octal(&mut header[124..136], data.len() as u64)?;
    write_octal(&mut header[136..148], 0)?; // mtime
    header[156] = b'0'; // regular file
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    // uname, gname, devmajor, devminor and prefix stay zeroed.
    // Checksum: the header with the checksum field treated as spaces.
    header[148..156].fill(b' ');
    let sum: u64 = header.iter().map(|&byte| u64::from(byte)).sum();
    write_octal(&mut header[149..156], sum)?;
    writer.write_all(&header)?;
    writer.write_all(data)?;
    let padding = (TAR_BLOCK_SIZE - (data.len() % TAR_BLOCK_SIZE)) % TAR_BLOCK_SIZE;
    if padding > 0 {
        writer.write_all(&[0u8; TAR_BLOCK_SIZE][..padding])?;
    }
    writer.write_all(&[0u8; TAR_BLOCK_SIZE * 2])?; // end-of-archive marker
    Ok(())
}

/// Writes a leading-zero octal number NUL-terminated into a fixed-width
/// field, the USTAR convention (e.g. mode 0o755 -> "0000755\0").
fn write_octal(field: &mut [u8], value: u64) -> io::Result<()> {
    let width = field.len() - 1;
    let mut remaining = value;
    for slot in field[..width].iter_mut().rev() {
        *slot = b'0' + (remaining % 8) as u8;
        remaining /= 8;
    }
    if remaining != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "value {value:#o} does not fit in {} octal digits",
                width - 1
            ),
        ));
    }
    field[width] = 0;
    Ok(())
}

/// Writes `data` as member `member` inside a ZIP archive, STORED
/// (uncompressed), with no extra fields, no comments and no timestamps
/// beyond the DOS epoch 1980-01-01 00:00:00.
pub fn write_zip(path: &Path, member: &str, data: &[u8]) -> io::Result<()> {
    write_zip_to(fs::File::create(path)?, member, data).map(|_| ())
}

fn write_zip_to<W: Write + Read + Seek>(mut writer: W, member: &str, data: &[u8]) -> io::Result<W> {
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o755)
        .system(zip::System::Unix);
    let mut archive = zip::ZipWriter::new(&mut writer);
    archive.start_file(member, options)?;
    archive.write_all(data)?;
    archive.finish()?;
    patch_zip_external_attr(&mut writer)?;
    Ok(writer)
}

/// Patches the central directory's external-attribute field to
/// `0o100755 << 16`.
///
/// `unix_permissions` masks mode bits to 0o777, dropping the S_IFREG bit of
/// the 0o100755 mode the release contract specifies. The central directory
/// offset is read from the end-of-central-directory record, so the patch does
/// not depend on any particular header layout.
fn patch_zip_external_attr<W: Write + Read + Seek>(writer: &mut W) -> io::Result<()> {
    let end = writer.stream_position()?;
    if end < 22 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "zip is too small to hold an end-of-central-directory record",
        ));
    }
    writer.seek(io::SeekFrom::Start(end - 22))?;
    let mut eocd = [0u8; 22];
    writer.read_exact(&mut eocd)?;
    if eocd[..4] != [0x50, 0x4b, 0x05, 0x06] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing end-of-central-directory record",
        ));
    }
    let central_offset = u64::from(u32::from_le_bytes([eocd[16], eocd[17], eocd[18], eocd[19]]));
    // A central directory entry is: 4-byte signature, then 34 bytes of fixed
    // fields, then the external-attribute field (APPNOTE 4.3.12).
    writer.seek(io::SeekFrom::Start(central_offset + 4 + 34))?;
    writer.write_all(&(0o100755u32 << 16).to_le_bytes())?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A payload spanning several tar blocks with varied bytes.
    fn payload() -> Vec<u8> {
        let mut bytes = Vec::with_capacity(3000);
        for i in 0..3000u32 {
            bytes.push(((i * 31 + (i >> 3)) & 0xff) as u8);
        }
        bytes
    }

    #[test]
    fn tar_gz_is_deterministic() {
        let data = payload();
        let first = write_tar_gz_to(Vec::new(), "bran", &data).unwrap();
        let second = write_tar_gz_to(Vec::new(), "bran", &data).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn zip_is_deterministic() {
        let data = payload();
        let first = write_zip_to(io::Cursor::new(Vec::new()), "bran.exe", &data)
            .unwrap()
            .into_inner();
        let second = write_zip_to(io::Cursor::new(Vec::new()), "bran.exe", &data)
            .unwrap()
            .into_inner();
        assert_eq!(first, second);
    }

    #[test]
    fn tar_gz_has_expected_member_metadata() {
        let data = payload();
        let gzip = write_tar_gz_to(Vec::new(), "bran", &data).unwrap();
        // gzip header: magic, deflate, no flags (no filename/extra/comment),
        // mtime 0.
        assert_eq!(&gzip[..2], &[0x1f, 0x8b]);
        assert_eq!(gzip[2], 8);
        assert_eq!(gzip[3], 0);
        assert_eq!(&gzip[4..8], &[0, 0, 0, 0]);

        let mut tar = Vec::new();
        flate2::read::GzDecoder::new(&gzip[..])
            .read_to_end(&mut tar)
            .unwrap();
        assert_eq!(tar.len(), 512 + padded_len(data.len()) + 1024);

        let header = &tar[..512];
        assert_eq!(header_name(header), "bran");
        assert_eq!(parse_octal(&header[100..108]), 0o755); // mode
        assert_eq!(parse_octal(&header[108..116]), 0); // uid
        assert_eq!(parse_octal(&header[116..124]), 0); // gid
        assert_eq!(parse_octal(&header[124..136]), data.len() as u64);
        assert_eq!(parse_octal(&header[136..148]), 0); // mtime
        assert_eq!(header[156], b'0'); // regular file
        assert_eq!(&header[257..263], b"ustar\0");
        assert_eq!(&header[263..265], b"00");
        assert!(header[265..297].iter().all(|&b| b == 0)); // uname
        assert!(header[297..329].iter().all(|&b| b == 0)); // gname
        assert_eq!(&tar[512..512 + data.len()], &data[..]);
        // checksum: header with the checksum field treated as spaces.
        let mut sum: u64 = 0;
        for (i, &byte) in header.iter().enumerate() {
            sum += if (148..156).contains(&i) {
                0x20
            } else {
                u64::from(byte)
            };
        }
        assert_eq!(parse_octal(&header[148..156]), sum);
        // padding and the end-of-archive marker are zero.
        assert!(tar[512 + data.len()..].iter().all(|&b| b == 0));
    }

    #[test]
    fn zip_has_expected_entry_properties() {
        let data = payload();
        let archive = write_zip_to(io::Cursor::new(Vec::new()), "bran.exe", &data)
            .unwrap()
            .into_inner();
        let (offset, count) = central_directory(&archive);
        assert_eq!(count, 1);
        let entry = &archive[offset..];
        assert_eq!(&entry[..4], &[0x50, 0x4b, 0x01, 0x02]);
        let version_made_by = u16::from_le_bytes([entry[4], entry[5]]);
        assert_eq!(version_made_by >> 8, 3); // create_system: Unix
        let method = u16::from_le_bytes([entry[10], entry[11]]);
        assert_eq!(method, 0); // STORED
        let time = u16::from_le_bytes([entry[12], entry[13]]);
        let date = u16::from_le_bytes([entry[14], entry[15]]);
        assert_eq!(time, 0);
        assert_eq!(date, 0x21); // DOS epoch: 1980-01-01 00:00:00
        let size = u32::from_le_bytes([entry[24], entry[25], entry[26], entry[27]]);
        assert_eq!(size, data.len() as u32);
        let name_len = u16::from_le_bytes([entry[28], entry[29]]);
        let extra_len = u16::from_le_bytes([entry[30], entry[31]]);
        let comment_len = u16::from_le_bytes([entry[32], entry[33]]);
        assert_eq!(&entry[46..46 + name_len as usize], b"bran.exe");
        assert_eq!(extra_len, 0);
        assert_eq!(comment_len, 0);
        let external = u32::from_le_bytes([entry[38], entry[39], entry[40], entry[41]]);
        assert_eq!(external, 0o100755 << 16);
        // the archive comment is empty
        assert_eq!(&archive[archive.len() - 2..], &[0, 0]);
    }

    fn padded_len(n: usize) -> usize {
        n.div_ceil(512) * 512
    }

    fn header_name(header: &[u8]) -> String {
        let end = header[..100].iter().position(|&b| b == 0).unwrap_or(100);
        String::from_utf8(header[..end].to_vec()).unwrap()
    }

    fn parse_octal(field: &[u8]) -> u64 {
        let s: String = field
            .iter()
            .map(|&b| b as char)
            .skip_while(|&c| c == ' ')
            .take_while(|&c| c != '\0' && c != ' ')
            .collect();
        u64::from_str_radix(&s, 8).unwrap()
    }

    /// Returns the offset and entry count of the central directory.
    fn central_directory(archive: &[u8]) -> (usize, usize) {
        let eocd = archive.len() - 22;
        assert_eq!(&archive[eocd..eocd + 4], &[0x50, 0x4b, 0x05, 0x06]);
        let count = u16::from_le_bytes([archive[eocd + 10], archive[eocd + 11]]) as usize;
        let offset = u32::from_le_bytes([
            archive[eocd + 16],
            archive[eocd + 17],
            archive[eocd + 18],
            archive[eocd + 19],
        ]) as usize;
        (offset, count)
    }
}
