use std::{
    fs::File,
    io::{self, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::{Error, Result, Stage};

use super::{ContainerVersion, path::normalize_archive_path};

const MAGIC_LENGTH: u32 = 8;
const MAX_ENTRY_COUNT: u32 = 1_000_000;
const MAX_PATH_LENGTH: u32 = 16_384;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MpkgEntry {
    pub path: String,
    pub offset: u64,
    pub size: u64,
}

/// Shared on-disk directory for isomorphic PKGV/PKGM containers.
#[derive(Debug)]
pub(crate) struct PackageDirectory {
    pub magic: [u8; MAGIC_LENGTH as usize],
    pub payload_start: u64,
    pub entries: Vec<MpkgEntry>,
    pub by_path: IndexMap<String, usize>,
}

/// Maps format / I/O failures for a specific package flavour.
pub(crate) struct PackageErrorDomain {
    pub stage: Stage,
    pub format: fn(String) -> Error,
}

impl PackageErrorDomain {
    pub const fn mpkg() -> Self {
        Self {
            stage: Stage::Verify,
            format: invalid_mpkg_reason,
        }
    }

    pub const fn desktop_pkg() -> Self {
        Self {
            stage: Stage::Unpack,
            format: invalid_desktop_package,
        }
    }

    pub(crate) fn format(&self, reason: impl Into<String>) -> Error {
        (self.format)(reason.into())
    }

    pub(crate) fn io(&self, path: &Path, source: io::Error) -> Error {
        Error::Io {
            stage: self.stage,
            path: path.to_path_buf(),
            source,
        }
    }
}

fn invalid_mpkg_reason(reason: String) -> Error {
    Error::invalid_mpkg(reason)
}

fn invalid_desktop_package(reason: String) -> Error {
    Error::InvalidProject { reason }
}

/// Read the length-prefixed magic + entry table + payload bounds.
///
/// Layout (little-endian):
/// `u32 magic_len | magic[magic_len] | u32 entry_count | entries... | payload`
/// where each entry is `u32 path_len | path | u32 offset | u32 size` and offsets
/// are relative to the payload start (immediately after the directory).
pub(crate) fn read_package_directory(
    path: &Path,
    domain: &PackageErrorDomain,
) -> Result<PackageDirectory> {
    let file = File::open(path).map_err(|source| domain.io(path, source))?;
    let file_len = file
        .metadata()
        .map_err(|source| domain.io(path, source))?
        .len();
    let mut reader = BufReader::new(file);

    let magic_length = read_u32(&mut reader, path, "magic length", domain)?;
    if magic_length != MAGIC_LENGTH {
        return Err(domain.format(format!(
            "magic length must be {MAGIC_LENGTH}, got {magic_length}"
        )));
    }
    let mut magic = [0_u8; MAGIC_LENGTH as usize];
    read_format_exact(&mut reader, &mut magic, path, "magic", domain)?;
    let entry_count = read_u32(&mut reader, path, "entry count", domain)?;
    if entry_count > MAX_ENTRY_COUNT {
        return Err(domain.format(format!(
            "entry count {entry_count} exceeds {MAX_ENTRY_COUNT}"
        )));
    }

    let capacity = usize::try_from(entry_count)
        .map_err(|_| domain.format("entry count does not fit this platform"))?;
    let mut entries = Vec::with_capacity(capacity);
    let mut by_path = IndexMap::with_capacity(capacity);
    for index in 0..entry_count {
        let path_length = read_u32(&mut reader, path, "entry path length", domain)?;
        if path_length == 0 || path_length > MAX_PATH_LENGTH {
            return Err(domain.format(format!(
                "entry {index} path length {path_length} is outside 1..={MAX_PATH_LENGTH}"
            )));
        }
        let path_capacity = usize::try_from(path_length)
            .map_err(|_| domain.format("entry path length does not fit this platform"))?;
        let mut path_bytes = vec![0_u8; path_capacity];
        read_format_exact(&mut reader, &mut path_bytes, path, "entry path", domain)?;
        let path_text = std::str::from_utf8(&path_bytes)
            .map_err(|_| domain.format(format!("entry {index} path is not UTF-8")))?;
        let entry_path = normalize_archive_path(path_text)
            .map_err(|bad| domain.format(format!("unsafe archive path: {bad:?}")))?;
        if by_path.contains_key(&entry_path) {
            return Err(domain.format(format!("duplicate archive path: {entry_path}")));
        }
        let offset = u64::from(read_u32(&mut reader, path, "entry offset", domain)?);
        let size = u64::from(read_u32(&mut reader, path, "entry size", domain)?);
        by_path.insert(entry_path.clone(), entries.len());
        entries.push(MpkgEntry {
            path: entry_path,
            offset,
            size,
        });
    }

    let payload_start = reader
        .stream_position()
        .map_err(|source| domain.io(path, source))?;
    let payload_len = file_len
        .checked_sub(payload_start)
        .ok_or_else(|| domain.format("directory exceeds file length"))?;
    validate_ranges(&entries, payload_len, domain)?;

    Ok(PackageDirectory {
        magic,
        payload_start,
        entries,
        by_path,
    })
}

pub(crate) fn read_package_entry_bytes(
    source: &Path,
    payload_start: u64,
    entry: &MpkgEntry,
    domain: &PackageErrorDomain,
) -> Result<Vec<u8>> {
    let absolute_offset = payload_start
        .checked_add(entry.offset)
        .ok_or_else(|| domain.format("entry absolute offset overflow"))?;
    let length = usize::try_from(entry.size)
        .map_err(|_| domain.format("entry size does not fit this platform"))?;
    let mut file = File::open(source).map_err(|source_err| domain.io(source, source_err))?;
    file.seek(SeekFrom::Start(absolute_offset))
        .map_err(|source_err| domain.io(source, source_err))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| domain.format("entry is too large to allocate"))?;
    bytes.resize(length, 0);
    read_format_exact(&mut file, &mut bytes, source, "entry payload", domain)?;
    Ok(bytes)
}

pub(crate) fn verify_package_entry_payload(
    source: &Path,
    payload_start: u64,
    entry: &MpkgEntry,
    path: &str,
    domain: &PackageErrorDomain,
) -> Result<()> {
    let absolute_offset = payload_start
        .checked_add(entry.offset)
        .ok_or_else(|| domain.format("entry absolute offset overflow"))?;
    let mut file = File::open(source).map_err(|source_err| domain.io(source, source_err))?;
    file.seek(SeekFrom::Start(absolute_offset))
        .map_err(|source_err| domain.io(source, source_err))?;
    let copied = io::copy(&mut file.take(entry.size), &mut io::sink())
        .map_err(|source_err| domain.io(source, source_err))?;
    if copied != entry.size {
        return Err(domain.format(format!(
            "unexpected EOF while reading entry payload: {path}"
        )));
    }
    Ok(())
}

#[derive(Debug)]
pub struct MpkgArchive {
    source: PathBuf,
    version: ContainerVersion,
    payload_start: u64,
    entries: Vec<MpkgEntry>,
    by_path: IndexMap<String, usize>,
}

impl MpkgArchive {
    pub fn open(path: &Path) -> Result<Self> {
        let domain = PackageErrorDomain::mpkg();
        let directory = read_package_directory(path, &domain)?;
        let version = ContainerVersion::from_magic(&directory.magic)?;
        Ok(Self {
            source: path.to_path_buf(),
            version,
            payload_start: directory.payload_start,
            entries: directory.entries,
            by_path: directory.by_path,
        })
    }

    pub const fn version(&self) -> ContainerVersion {
        self.version
    }

    pub fn entries(&self) -> &[MpkgEntry] {
        &self.entries
    }

    pub fn read_entry(&self, path: &str) -> Result<Vec<u8>> {
        let entry = self.entry_by_path(path)?;
        read_package_entry_bytes(
            &self.source,
            self.payload_start,
            entry,
            &PackageErrorDomain::mpkg(),
        )
    }

    pub(crate) fn verify_entry_payload(&self, path: &str) -> Result<()> {
        let entry = self.entry_by_path(path)?;
        verify_package_entry_payload(
            &self.source,
            self.payload_start,
            entry,
            path,
            &PackageErrorDomain::mpkg(),
        )
    }

    fn entry_by_path(&self, path: &str) -> Result<&MpkgEntry> {
        let index = self
            .by_path
            .get(path)
            .copied()
            .ok_or_else(|| Error::invalid_mpkg(format!("archive entry not found: {path}")))?;
        Ok(&self.entries[index])
    }
}

fn validate_ranges(
    entries: &[MpkgEntry],
    payload_len: u64,
    domain: &PackageErrorDomain,
) -> Result<()> {
    let mut ranges = Vec::with_capacity(entries.len());
    for entry in entries {
        let end = entry
            .offset
            .checked_add(entry.size)
            .ok_or_else(|| domain.format("entry range overflow"))?;
        if end > payload_len {
            return Err(domain.format(format!(
                "entry {} range {}..{} exceeds payload length {payload_len}",
                entry.path, entry.offset, end
            )));
        }
        if entry.size != 0 {
            ranges.push((entry.offset, end, entry.path.as_str()));
        }
    }
    ranges.sort_unstable_by_key(|(offset, _, _)| *offset);
    let mut previous_end = 0;
    let mut previous_path = None;
    for (offset, end, path) in ranges {
        if offset < previous_end {
            return Err(domain.format(format!(
                "entry {path} overlaps {}",
                previous_path.unwrap_or("an earlier entry")
            )));
        }
        previous_end = end;
        previous_path = Some(path);
    }
    Ok(())
}

fn read_u32(
    reader: &mut impl Read,
    path: &Path,
    label: &str,
    domain: &PackageErrorDomain,
) -> Result<u32> {
    let mut bytes = [0_u8; 4];
    read_format_exact(reader, &mut bytes, path, label, domain)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_format_exact(
    reader: &mut impl Read,
    buffer: &mut [u8],
    path: &Path,
    label: &str,
    domain: &PackageErrorDomain,
) -> Result<()> {
    reader.read_exact(buffer).map_err(|source| {
        if source.kind() == io::ErrorKind::UnexpectedEof {
            domain.format(format!("unexpected EOF while reading {label}"))
        } else {
            domain.io(path, source)
        }
    })
}
