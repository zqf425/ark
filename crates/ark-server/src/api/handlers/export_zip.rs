//! Deterministic zip packaging for thread exports (`?format=zip`).
//!
//! Only what the export route needs: a single-entry, uncompressed (`stored`)
//! archive with a fixed MS-DOS timestamp, so identical store state still
//! produces byte-identical archives — the same determinism rule the bare JSON
//! export already follows. Storing instead of deflating keeps the writer
//! dependency-free apart from CRC-32, and snapshot JSON is small enough that
//! compression is irrelevant.

use crc32fast::Hasher;

/// Fixed MS-DOS timestamp: the zip epoch minimum, 1980-01-01 00:00:00.
/// A real clock here would make identical exports differ byte-for-byte.
const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = 0x0021;

const LOCAL_HEADER_SIG: u32 = 0x0403_4b50;
const CENTRAL_HEADER_SIG: u32 = 0x0201_4b50;
const EOCD_SIG: u32 = 0x0605_4b50;
const ZIP_VERSION: u16 = 20;
const STORED_METHOD: u16 = 0;

/// Builds a one-entry stored zip archive containing `contents` under `name`.
pub fn zip_single_entry(name: &str, contents: &[u8]) -> Vec<u8> {
    let name_bytes = name.as_bytes();
    let name_len = name_bytes.len() as u16;
    let size = contents.len() as u32;
    let crc = crc32(contents);

    let mut out =
        Vec::with_capacity(30 + name_bytes.len() + contents.len() + 46 + name_bytes.len() + 22);

    // Local file header. The archive starts at offset 0, so the entry's local
    // header offset written into the central directory is always zero.
    out.extend_from_slice(&LOCAL_HEADER_SIG.to_le_bytes());
    out.extend_from_slice(&ZIP_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // no general-purpose flags
    out.extend_from_slice(&STORED_METHOD.to_le_bytes());
    out.extend_from_slice(&DOS_TIME.to_le_bytes());
    out.extend_from_slice(&DOS_DATE.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes()); // compressed == stored size
    out.extend_from_slice(&size.to_le_bytes()); // uncompressed
    out.extend_from_slice(&name_len.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // extra field length
    out.extend_from_slice(name_bytes);
    out.extend_from_slice(contents);

    // Central directory header.
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&CENTRAL_HEADER_SIG.to_le_bytes());
    out.extend_from_slice(&ZIP_VERSION.to_le_bytes()); // version made by
    out.extend_from_slice(&ZIP_VERSION.to_le_bytes()); // version needed
    out.extend_from_slice(&0u16.to_le_bytes()); // flags
    out.extend_from_slice(&STORED_METHOD.to_le_bytes());
    out.extend_from_slice(&DOS_TIME.to_le_bytes());
    out.extend_from_slice(&DOS_DATE.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&name_len.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // extra
    out.extend_from_slice(&0u16.to_le_bytes()); // comment
    out.extend_from_slice(&0u16.to_le_bytes()); // disk number start
    out.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
    out.extend_from_slice(&0u32.to_le_bytes()); // external attributes
    out.extend_from_slice(&0u32.to_le_bytes()); // local header offset
    out.extend_from_slice(name_bytes);

    // End of central directory.
    let cd_size = out.len() as u32 - cd_offset;
    out.extend_from_slice(&EOCD_SIG.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // this disk
    out.extend_from_slice(&0u16.to_le_bytes()); // cd start disk
    out.extend_from_slice(&1u16.to_le_bytes()); // entries on this disk
    out.extend_from_slice(&1u16.to_le_bytes()); // entries total
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut hasher = Hasher::new();
    hasher.update(data);
    hasher.finalize()
}

/// Test-only reader so route tests can assert on the wrapped payload without
/// pulling a full unzip implementation into the tree.
#[cfg(test)]
pub(crate) fn read_single_entry(archive: &[u8]) -> Option<(String, Vec<u8>)> {
    let read_u16 = |offset: usize| -> Option<u16> {
        let bytes = archive.get(offset..offset + 2)?;
        Some(u16::from_le_bytes(bytes.try_into().ok()?))
    };
    let read_u32 = |offset: usize| -> Option<u32> {
        let bytes = archive.get(offset..offset + 4)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    };

    // Walk backwards to the EOCD (no comment is ever written, so it is the
    // last 22 bytes, but the scan keeps the helper honest about the format).
    let mut eocd = None;
    let tail_start = archive.len().saturating_sub(22 + u16::MAX as usize);
    for offset in (tail_start..archive.len().saturating_sub(21)).rev() {
        if read_u32(offset) == Some(EOCD_SIG) {
            eocd = Some(offset);
            break;
        }
    }
    let eocd = eocd?;
    let entries = read_u16(eocd + 10)?;
    let cd_offset = read_u32(eocd + 16)? as usize;
    if entries != 1 {
        return None;
    }

    let name_len = read_u16(cd_offset + 28)? as usize;
    let extra_len = read_u16(cd_offset + 30)? as usize;
    let comment_len = read_u16(cd_offset + 32)? as usize;
    let local_offset = read_u32(cd_offset + 42)? as usize;
    let name = core::str::from_utf8(archive.get(cd_offset + 46..cd_offset + 46 + name_len)?)
        .ok()?
        .to_string();

    let local_name_len = read_u16(local_offset + 26)? as usize;
    let local_extra_len = read_u16(local_offset + 28)? as usize;
    let data_start = local_offset + 30 + local_name_len + local_extra_len;
    let size = read_u32(local_offset + 18)? as usize;
    let contents = archive.get(data_start..data_start + size)?.to_vec();
    let _ = extra_len + comment_len;
    Some((name, contents))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_starts_with_the_local_header_signature() {
        let zip = zip_single_entry("a.json", b"{}");
        assert_eq!(&zip[..4], b"PK\x03\x04");
        // EOCD is the final 22-byte record; its signature leads the record.
        assert_eq!(&zip[zip.len() - 22..zip.len() - 18], b"PK\x05\x06");
    }

    #[test]
    fn entry_round_trips_through_the_test_reader() {
        // Build via str::as_bytes: byte literals reject non-ASCII chars, while
        // str literals keep the UTF-8 payload readable.
        let payload = "{\"format_version\":1,\"title\":\"thread-快照\"}"
            .as_bytes()
            .to_vec();
        let (name, contents) = read_single_entry(&zip_single_entry("thread-x.json", &payload))
            .expect("single entry archive");
        assert_eq!(name, "thread-x.json");
        assert_eq!(contents, payload);
    }

    #[test]
    fn identical_inputs_produce_byte_identical_archives() {
        let first = zip_single_entry("same.json", b"payload");
        let second = zip_single_entry("same.json", b"payload");
        assert_eq!(first, second, "no timestamp may leak into the archive");
    }
}
