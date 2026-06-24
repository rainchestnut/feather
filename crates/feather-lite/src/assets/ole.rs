//! Minimal OLE Compound File Binary stream reader.
//!
//! Several private CAD formats store preview/cache streams inside CFB/OLE
//! containers. This reader reconstructs regular FAT streams and mini-streams so
//! the existing lightweight asset scanners can inspect stream payloads.

use crate::importer::{ImportError, ImportLimits};

const HEADER_LEN: usize = 512;
const OLE_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const FREESECT: u32 = 0xFFFF_FFFF;
const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
const FATSECT: u32 = 0xFFFF_FFFD;
const DIFSECT: u32 = 0xFFFF_FFFC;
const FIRST_DIFAT_OFFSET: usize = 0x4C;
const FIRST_DIFAT_LEN: usize = 109;
const DIRECTORY_ENTRY_LEN: usize = 128;
const MINI_STREAM_CUTOFF_DEFAULT: u64 = 4096;

/// Reconstructed stream from an OLE/CFB container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OleStream {
    pub name: String,
    pub payload: Vec<u8>,
}

/// Extracts regular and mini streams from an OLE/CFB byte buffer while
/// enforcing limits before stream payloads are reconstructed.
pub fn extract_ole_streams(
    bytes: &[u8],
    limits: &ImportLimits,
) -> Result<Vec<OleStream>, ImportError> {
    if !is_ole_container(bytes) {
        return Ok(Vec::new());
    }
    if bytes.len() < HEADER_LEN {
        return Err(ImportError::InvalidData(
            "OLE header is truncated".to_string(),
        ));
    }
    if read_u16(bytes, 0x1C)? != 0xFFFE {
        return Err(ImportError::InvalidData(
            "OLE byte order is not little-endian".to_string(),
        ));
    }

    let sector_shift = read_u16(bytes, 0x1E)? as usize;
    let mini_sector_shift = read_u16(bytes, 0x20)? as usize;
    let sector_size = 1_usize
        .checked_shl(sector_shift as u32)
        .ok_or_else(|| ImportError::InvalidData("OLE sector size overflows".to_string()))?;
    if !(512..=4096).contains(&sector_size) || !sector_size.is_power_of_two() {
        return Err(ImportError::InvalidData(format!(
            "unsupported OLE sector size {sector_size}"
        )));
    }
    let mini_sector_size = 1_usize
        .checked_shl(mini_sector_shift as u32)
        .ok_or_else(|| ImportError::InvalidData("OLE mini sector size overflows".to_string()))?;
    if mini_sector_size == 0
        || mini_sector_size > sector_size
        || !mini_sector_size.is_power_of_two()
    {
        return Err(ImportError::InvalidData(format!(
            "unsupported OLE mini sector size {mini_sector_size}"
        )));
    }

    let first_directory_sector = read_u32(bytes, 0x30)?;
    let fat_sector_count = read_u32(bytes, 0x2C)? as usize;
    let mini_stream_cutoff = read_u32(bytes, 0x38)? as u64;
    let first_mini_fat_sector = read_u32(bytes, 0x3C)?;
    let mini_fat_sector_count = read_u32(bytes, 0x40)? as usize;
    let first_difat_sector = read_u32(bytes, 0x44)?;
    let difat_sector_count = read_u32(bytes, 0x48)? as usize;
    let container_sector_count = bytes.len().saturating_sub(HEADER_LEN) / sector_size;
    if fat_sector_count > container_sector_count {
        return Err(ImportError::InvalidData(format!(
            "OLE FAT sector count {fat_sector_count} exceeds container sector count {container_sector_count}"
        )));
    }
    if difat_sector_count > container_sector_count {
        return Err(ImportError::InvalidData(format!(
            "OLE DIFAT sector count {difat_sector_count} exceeds container sector count {container_sector_count}"
        )));
    }
    let fat_sector_ids = read_difat(
        bytes,
        sector_size,
        first_difat_sector,
        difat_sector_count,
        fat_sector_count,
        container_sector_count,
    )?;
    let fat = read_fat(bytes, sector_size, &fat_sector_ids)?;
    let directory_bytes =
        read_regular_stream_chain(bytes, sector_size, &fat, first_directory_sector, usize::MAX)?;
    let directory_entries = parse_directory_entries(&directory_bytes)?;
    validate_stream_limits(&directory_entries, limits)?;
    let root_entry = directory_entries
        .iter()
        .find(|entry| entry.object_type == DirectoryObjectType::Root);

    let cutoff = if mini_stream_cutoff == 0 {
        MINI_STREAM_CUTOFF_DEFAULT
    } else {
        mini_stream_cutoff
    };

    let mini_fat =
        if matches!(first_mini_fat_sector, FREESECT | ENDOFCHAIN) || mini_fat_sector_count == 0 {
            Vec::new()
        } else {
            read_mini_fat(
                bytes,
                sector_size,
                &fat,
                first_mini_fat_sector,
                mini_fat_sector_count,
            )?
        };
    let mini_stream = if let Some(root_entry) = root_entry {
        read_regular_stream_chain(
            bytes,
            sector_size,
            &fat,
            root_entry.start_sector,
            usize::try_from(root_entry.size).map_err(|_| {
                ImportError::InvalidData("OLE root mini stream size overflows".to_string())
            })?,
        )?
    } else {
        Vec::new()
    };

    let mut streams = Vec::new();
    for directory_entry in directory_entries {
        if directory_entry.object_type != DirectoryObjectType::Stream {
            continue;
        }
        if directory_entry.size == 0 {
            continue;
        }

        let payload = if directory_entry.size < cutoff {
            if mini_fat.is_empty() || mini_stream.is_empty() {
                continue;
            }
            read_mini_stream_chain(
                &mini_stream,
                mini_sector_size,
                &mini_fat,
                directory_entry.start_sector,
                usize::try_from(directory_entry.size).map_err(|_| {
                    ImportError::InvalidData("OLE mini stream size overflows".to_string())
                })?,
            )?
        } else {
            read_regular_stream_chain(
                bytes,
                sector_size,
                &fat,
                directory_entry.start_sector,
                usize::try_from(directory_entry.size).map_err(|_| {
                    ImportError::InvalidData("OLE stream size overflows".to_string())
                })?,
            )?
        };
        streams.push(OleStream {
            name: directory_entry.name,
            payload,
        });
    }

    Ok(streams)
}

fn validate_stream_limits(
    directory_entries: &[DirectoryEntry],
    limits: &ImportLimits,
) -> Result<(), ImportError> {
    let mut stream_count = 0_usize;
    let mut total_stream_bytes = 0_u64;
    let stream_byte_limit = u64::try_from(limits.max_ole_stream_bytes).unwrap_or(u64::MAX);
    let total_byte_limit = u64::try_from(limits.max_ole_total_stream_bytes).unwrap_or(u64::MAX);

    for entry in directory_entries {
        if entry.object_type != DirectoryObjectType::Stream || entry.size == 0 {
            continue;
        }

        stream_count = stream_count.saturating_add(1);
        if stream_count > limits.max_ole_streams {
            return Err(ImportError::ResourceLimitExceeded {
                resource: "OLE stream count",
                limit: limits.max_ole_streams,
                actual: stream_count,
            });
        }

        let actual = usize::try_from(entry.size).unwrap_or(usize::MAX);
        if entry.size > stream_byte_limit {
            return Err(ImportError::ResourceLimitExceeded {
                resource: "OLE stream bytes",
                limit: limits.max_ole_stream_bytes,
                actual,
            });
        }

        total_stream_bytes = total_stream_bytes.saturating_add(entry.size);
        if total_stream_bytes > total_byte_limit {
            return Err(ImportError::ResourceLimitExceeded {
                resource: "OLE total stream bytes",
                limit: limits.max_ole_total_stream_bytes,
                actual: usize::try_from(total_stream_bytes).unwrap_or(usize::MAX),
            });
        }
    }

    Ok(())
}

fn is_ole_container(bytes: &[u8]) -> bool {
    bytes.len() >= OLE_MAGIC.len() && bytes[..OLE_MAGIC.len()] == OLE_MAGIC
}

fn read_difat(
    bytes: &[u8],
    sector_size: usize,
    first_difat_sector: u32,
    difat_sector_count: usize,
    fat_sector_count: usize,
    container_sector_count: usize,
) -> Result<Vec<u32>, ImportError> {
    let mut sectors = Vec::with_capacity(fat_sector_count);
    for index in 0..FIRST_DIFAT_LEN {
        if sectors.len() == fat_sector_count {
            break;
        }
        let offset = FIRST_DIFAT_OFFSET + index * 4;
        let sector = read_u32(bytes, offset)?;
        if !matches!(sector, FREESECT | ENDOFCHAIN) {
            sectors.push(sector);
        }
    }

    let mut current = first_difat_sector;
    let mut visited = vec![false; container_sector_count];
    for _ in 0..difat_sector_count {
        if sectors.len() == fat_sector_count {
            break;
        }
        if matches!(current, FREESECT | ENDOFCHAIN) {
            break;
        }
        let current_index = current as usize;
        if current_index >= visited.len() {
            return Err(ImportError::InvalidData(format!(
                "OLE DIFAT chain references missing sector {current}"
            )));
        }
        if visited[current_index] {
            return Err(ImportError::InvalidData(
                "OLE DIFAT chain contains a cycle".to_string(),
            ));
        }
        visited[current_index] = true;

        let sector = sector_slice(bytes, sector_size, current)?;
        let entries_per_sector = sector_size / 4;
        for index in 0..entries_per_sector.saturating_sub(1) {
            if sectors.len() == fat_sector_count {
                break;
            }
            let value = read_u32(sector, index * 4)?;
            if !matches!(value, FREESECT | ENDOFCHAIN) {
                sectors.push(value);
            }
        }
        current = read_u32(sector, sector_size - 4)?;
    }

    if sectors.len() != fat_sector_count {
        return Err(ImportError::InvalidData(format!(
            "OLE FAT declares {fat_sector_count} sectors but DIFAT provides {}",
            sectors.len()
        )));
    }

    Ok(sectors)
}

fn read_fat(
    bytes: &[u8],
    sector_size: usize,
    fat_sector_ids: &[u32],
) -> Result<Vec<u32>, ImportError> {
    let mut fat = Vec::new();
    for sector_id in fat_sector_ids {
        if matches!(*sector_id, FREESECT | ENDOFCHAIN | DIFSECT) {
            continue;
        }
        let sector = sector_slice(bytes, sector_size, *sector_id)?;
        for offset in (0..sector_size).step_by(4) {
            fat.push(read_u32(sector, offset)?);
        }
    }
    Ok(fat)
}

fn read_mini_fat(
    bytes: &[u8],
    sector_size: usize,
    fat: &[u32],
    first_mini_fat_sector: u32,
    mini_fat_sector_count: usize,
) -> Result<Vec<u32>, ImportError> {
    let expected_size = mini_fat_sector_count
        .checked_mul(sector_size)
        .ok_or_else(|| ImportError::InvalidData("OLE mini FAT size overflows".to_string()))?;
    let mini_fat_bytes = read_regular_stream_chain(
        bytes,
        sector_size,
        fat,
        first_mini_fat_sector,
        expected_size,
    )?;

    let mut entries = Vec::new();
    for offset in (0..mini_fat_bytes.len()).step_by(4) {
        entries.push(read_u32(&mini_fat_bytes, offset)?);
    }
    Ok(entries)
}

fn read_regular_stream_chain(
    bytes: &[u8],
    sector_size: usize,
    fat: &[u32],
    start_sector: u32,
    expected_size: usize,
) -> Result<Vec<u8>, ImportError> {
    if matches!(start_sector, FREESECT | ENDOFCHAIN | FATSECT | DIFSECT) {
        return Ok(Vec::new());
    }

    let mut payload = Vec::new();
    let mut current = start_sector;
    let mut visited = vec![false; fat.len()];

    while !matches!(current, ENDOFCHAIN | FREESECT) {
        let current_index = current as usize;
        if current_index >= fat.len() {
            return Err(ImportError::InvalidData(format!(
                "OLE FAT chain references missing sector {current}"
            )));
        }
        if visited[current_index] {
            return Err(ImportError::InvalidData(
                "OLE FAT chain contains a cycle".to_string(),
            ));
        }
        visited[current_index] = true;

        let sector = sector_slice(bytes, sector_size, current)?;
        payload.extend_from_slice(sector);
        if payload.len() >= expected_size {
            payload.truncate(expected_size);
            return Ok(payload);
        }

        current = fat[current_index];
        if matches!(current, FATSECT | DIFSECT) {
            return Err(ImportError::InvalidData(
                "OLE stream chain points at metadata sector".to_string(),
            ));
        }
    }

    if expected_size != usize::MAX && payload.len() < expected_size {
        return Err(ImportError::InvalidData(
            "OLE stream chain ended before expected size".to_string(),
        ));
    }
    Ok(payload)
}

fn read_mini_stream_chain(
    mini_stream: &[u8],
    mini_sector_size: usize,
    mini_fat: &[u32],
    start_sector: u32,
    expected_size: usize,
) -> Result<Vec<u8>, ImportError> {
    if matches!(start_sector, FREESECT | ENDOFCHAIN | FATSECT | DIFSECT) {
        return Ok(Vec::new());
    }

    let mut payload = Vec::new();
    let mut current = start_sector;
    let mut visited = vec![false; mini_fat.len()];

    while !matches!(current, ENDOFCHAIN | FREESECT) {
        let current_index = current as usize;
        if current_index >= mini_fat.len() {
            return Err(ImportError::InvalidData(format!(
                "OLE mini FAT chain references missing mini sector {current}"
            )));
        }
        if visited[current_index] {
            return Err(ImportError::InvalidData(
                "OLE mini FAT chain contains a cycle".to_string(),
            ));
        }
        visited[current_index] = true;

        let sector = mini_sector_slice(mini_stream, mini_sector_size, current)?;
        payload.extend_from_slice(sector);
        if payload.len() >= expected_size {
            payload.truncate(expected_size);
            return Ok(payload);
        }

        current = mini_fat[current_index];
        if matches!(current, FATSECT | DIFSECT) {
            return Err(ImportError::InvalidData(
                "OLE mini stream chain points at metadata sector".to_string(),
            ));
        }
    }

    if payload.len() < expected_size {
        return Err(ImportError::InvalidData(
            "OLE mini stream chain ended before expected size".to_string(),
        ));
    }
    Ok(payload)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectoryObjectType {
    Unknown,
    Storage,
    Stream,
    Root,
}

struct DirectoryEntry {
    name: String,
    object_type: DirectoryObjectType,
    start_sector: u32,
    size: u64,
}

fn parse_directory_entries(bytes: &[u8]) -> Result<Vec<DirectoryEntry>, ImportError> {
    let mut entries = Vec::new();
    for entry in bytes.chunks_exact(DIRECTORY_ENTRY_LEN) {
        if let Some(directory_entry) = parse_directory_entry(entry)? {
            entries.push(directory_entry);
        }
    }
    Ok(entries)
}

fn parse_directory_entry(bytes: &[u8]) -> Result<Option<DirectoryEntry>, ImportError> {
    let name_len = read_u16(bytes, 64)? as usize;
    if !(2..=64).contains(&name_len) {
        return Ok(None);
    }

    let name_bytes = &bytes[..name_len - 2];
    let mut code_units = Vec::new();
    for chunk in name_bytes.chunks_exact(2) {
        code_units.push(u16::from_le_bytes([chunk[0], chunk[1]]));
    }
    let name = String::from_utf16(&code_units).map_err(|error| {
        ImportError::InvalidData(format!("OLE stream name is invalid: {error}"))
    })?;

    let object_type = match bytes[66] {
        1 => DirectoryObjectType::Storage,
        2 => DirectoryObjectType::Stream,
        5 => DirectoryObjectType::Root,
        _ => DirectoryObjectType::Unknown,
    };
    let start_sector = read_u32(bytes, 116)?;
    let size = read_u64(bytes, 120)?;

    Ok(Some(DirectoryEntry {
        name,
        object_type,
        start_sector,
        size,
    }))
}

fn sector_slice(bytes: &[u8], sector_size: usize, sector_id: u32) -> Result<&[u8], ImportError> {
    let start = HEADER_LEN
        .checked_add(
            (sector_id as usize)
                .checked_mul(sector_size)
                .ok_or_else(|| {
                    ImportError::InvalidData("OLE sector offset overflows".to_string())
                })?,
        )
        .ok_or_else(|| ImportError::InvalidData("OLE sector offset overflows".to_string()))?;
    let end = start
        .checked_add(sector_size)
        .ok_or_else(|| ImportError::InvalidData("OLE sector end overflows".to_string()))?;
    bytes
        .get(start..end)
        .ok_or_else(|| ImportError::InvalidData("OLE sector is truncated".to_string()))
}

fn mini_sector_slice(
    bytes: &[u8],
    mini_sector_size: usize,
    sector_id: u32,
) -> Result<&[u8], ImportError> {
    let start = (sector_id as usize)
        .checked_mul(mini_sector_size)
        .ok_or_else(|| ImportError::InvalidData("OLE mini sector offset overflows".to_string()))?;
    let end = start
        .checked_add(mini_sector_size)
        .ok_or_else(|| ImportError::InvalidData("OLE mini sector end overflows".to_string()))?;
    bytes
        .get(start..end)
        .ok_or_else(|| ImportError::InvalidData("OLE mini sector is truncated".to_string()))
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ImportError> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| ImportError::InvalidData("OLE u16 is truncated".to_string()))?;
    Ok(u16::from_le_bytes(value.try_into().map_err(|_| {
        ImportError::InvalidData("OLE u16 has invalid width".to_string())
    })?))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ImportError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| ImportError::InvalidData("OLE u32 is truncated".to_string()))?;
    Ok(u32::from_le_bytes(value.try_into().map_err(|_| {
        ImportError::InvalidData("OLE u32 has invalid width".to_string())
    })?))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ImportError> {
    let value = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| ImportError::InvalidData("OLE u64 is truncated".to_string()))?;
    Ok(u64::from_le_bytes(value.try_into().map_err(|_| {
        ImportError::InvalidData("OLE u64 has invalid width".to_string())
    })?))
}
