use std::io;

use serde::{Deserialize, Serialize};

const MAGIC: &[u8; 4] = b"GMXB";
const BUNDLE_VERSION: u16 = 1;
const HEADER_SIZE: usize = 8;
const TOC_ENTRY_SIZE: usize = 20;

const SECTION_CACHE: u16 = 1;
const SECTION_TARGETS: u16 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BundleTargetMetadata {
    pub targets: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct DecodedBundle {
    pub cache_payload: Vec<u8>,
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct TocEntry {
    kind: u16,
    offset: u64,
    len: u64,
}

pub(crate) fn encode(cache_payload: &[u8], targets: &[String]) -> io::Result<Vec<u8>> {
    let mut sections: Vec<(u16, Vec<u8>)> = vec![(SECTION_CACHE, cache_payload.to_vec())];

    if !targets.is_empty() {
        let target_bytes = bincode::serialize(&BundleTargetMetadata {
            targets: targets.to_vec(),
        })
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
        sections.push((SECTION_TARGETS, target_bytes));
    }

    let section_count = sections.len() as u16;
    let toc_size = TOC_ENTRY_SIZE
        .checked_mul(section_count as usize)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bundle TOC too large"))?;
    let mut offset = (HEADER_SIZE + toc_size) as u64;

    let mut entries = Vec::with_capacity(sections.len());
    for (kind, payload) in &sections {
        entries.push(TocEntry {
            kind: *kind,
            offset,
            len: payload.len() as u64,
        });
        offset = offset.checked_add(payload.len() as u64).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "bundle payload too large")
        })?;
    }

    let mut bytes = Vec::with_capacity(offset as usize);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&BUNDLE_VERSION.to_le_bytes());
    bytes.extend_from_slice(&section_count.to_le_bytes());

    for entry in &entries {
        bytes.extend_from_slice(&entry.kind.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&entry.offset.to_le_bytes());
        bytes.extend_from_slice(&entry.len.to_le_bytes());
    }

    for (_, payload) in sections {
        bytes.extend_from_slice(&payload);
    }

    Ok(bytes)
}

pub(crate) fn decode(bytes: &[u8]) -> io::Result<DecodedBundle> {
    if bytes.len() < HEADER_SIZE {
        return Err(format_error("bundle header is truncated"));
    }

    if &bytes[0..4] != MAGIC {
        return Err(format_error("invalid bundle magic, expected GMXB"));
    }

    let version = read_u16(bytes, 4)?;
    if version != BUNDLE_VERSION {
        return Err(format_error(&format!(
            "unsupported bundle version: {}",
            version
        )));
    }

    let section_count = read_u16(bytes, 6)? as usize;
    let toc_len = TOC_ENTRY_SIZE
        .checked_mul(section_count)
        .ok_or_else(|| format_error("bundle TOC length overflow"))?;
    let toc_end = HEADER_SIZE
        .checked_add(toc_len)
        .ok_or_else(|| format_error("bundle TOC bounds overflow"))?;

    if bytes.len() < toc_end {
        return Err(format_error("bundle TOC is truncated"));
    }

    let mut entries = Vec::with_capacity(section_count);
    let mut cursor = HEADER_SIZE;
    for _ in 0..section_count {
        let kind = read_u16(bytes, cursor)?;
        let offset = read_u64(bytes, cursor + 4)?;
        let len = read_u64(bytes, cursor + 12)?;
        let end = offset
            .checked_add(len)
            .ok_or_else(|| format_error("bundle section bounds overflow"))?;
        if end as usize > bytes.len() {
            return Err(format_error("bundle section is out of bounds"));
        }
        entries.push(TocEntry { kind, offset, len });
        cursor += TOC_ENTRY_SIZE;
    }

    let mut cache_payload: Option<Vec<u8>> = None;
    let mut targets = Vec::new();

    for entry in entries {
        let start = entry.offset as usize;
        let end = start + entry.len as usize;
        let data = &bytes[start..end];

        match entry.kind {
            SECTION_CACHE => {
                if cache_payload.is_some() {
                    return Err(format_error("bundle contains multiple cache sections"));
                }
                cache_payload = Some(data.to_vec());
            }
            SECTION_TARGETS => {
                let metadata: BundleTargetMetadata = bincode::deserialize(data)
                    .map_err(|err| format_error(&format!("invalid targets section: {}", err)))?;
                targets = metadata.targets;
            }
            _ => {
                // Reserved for future extension.
            }
        }
    }

    let cache_payload =
        cache_payload.ok_or_else(|| format_error("bundle is missing cache section"))?;

    Ok(DecodedBundle {
        cache_payload,
        targets,
    })
}

fn read_u16(bytes: &[u8], offset: usize) -> io::Result<u16> {
    let end = offset
        .checked_add(2)
        .ok_or_else(|| format_error("bundle integer read overflow"))?;
    let raw = bytes
        .get(offset..end)
        .ok_or_else(|| format_error("bundle integer read out of bounds"))?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> io::Result<u64> {
    let end = offset
        .checked_add(8)
        .ok_or_else(|| format_error("bundle integer read overflow"))?;
    let raw = bytes
        .get(offset..end)
        .ok_or_else(|| format_error("bundle integer read out of bounds"))?;
    Ok(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

fn format_error(message: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid grammar bundle: {}", message),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_bundle_without_targets() {
        let bytes = encode(b"cache-bytes", &[]).expect("bundle should encode");
        let decoded = decode(&bytes).expect("bundle should decode");
        assert_eq!(decoded.cache_payload, b"cache-bytes");
        assert!(decoded.targets.is_empty());
    }

    #[test]
    fn rejects_non_bundle_magic() {
        let err = decode(b"legacy-bytes").expect_err("non-bundle input must be rejected");
        assert!(err.to_string().contains("invalid grammar bundle"));
        assert!(err.to_string().contains("GMXB"));
    }
}
