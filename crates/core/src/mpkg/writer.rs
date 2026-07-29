use std::{
    fs::{self, File},
    io::{self, BufWriter, Read, Write},
    path::{Path, PathBuf},
};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::{Error, Result, Stage};

use super::{ContainerVersion, MpkgArchive, path::validate_archive_path};

const HEADER_SIZE: u64 = 4 + 8 + 4;
const MAX_PATH_LENGTH: usize = 16_384;
const FOUR_GIB: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Debug)]
enum EntrySource {
    Bytes(Vec<u8>),
    File { path: PathBuf, size: u64 },
}

impl EntrySource {
    fn size(&self) -> u64 {
        match self {
            Self::Bytes(bytes) => bytes.len() as u64,
            Self::File { size, .. } => *size,
        }
    }
}

#[derive(Debug)]
struct PendingEntry {
    archive_path: String,
    source: EntrySource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverwritePolicy {
    Deny,
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteReport {
    pub output: PathBuf,
    pub version: ContainerVersion,
    pub entries: usize,
    pub bytes: u64,
}

#[derive(Debug)]
pub struct MpkgBuilder {
    version: ContainerVersion,
    entries: Vec<PendingEntry>,
    paths: IndexMap<String, ()>,
}

impl MpkgBuilder {
    pub fn new(version: ContainerVersion) -> Self {
        Self {
            version,
            entries: Vec::new(),
            paths: IndexMap::new(),
        }
    }

    pub fn add_bytes(&mut self, path: &str, bytes: Vec<u8>) -> Result<()> {
        let archive_path = self.validate_new_path(path)?;
        self.paths.insert(archive_path.clone(), ());
        self.entries.push(PendingEntry {
            archive_path,
            source: EntrySource::Bytes(bytes),
        });
        Ok(())
    }

    pub fn add_file(&mut self, path: &str, source: &Path) -> Result<()> {
        let archive_path = self.validate_new_path(path)?;
        let metadata = fs::metadata(source).map_err(|error| pack_io(source, error))?;
        if !metadata.is_file() {
            return Err(Error::ConversionFailed {
                reason: format!("entry source is not a file: {}", source.display()),
            });
        }
        self.paths.insert(archive_path.clone(), ());
        self.entries.push(PendingEntry {
            archive_path,
            source: EntrySource::File {
                path: source.to_path_buf(),
                size: metadata.len(),
            },
        });
        Ok(())
    }

    pub fn write_atomic(&self, output: &Path, overwrite: OverwritePolicy) -> Result<WriteReport> {
        self.write_atomic_verified(output, overwrite, |_| Ok(()))
    }

    /// Write a staged MPKG, run generic staged verification, then invoke
    /// `verify` on the staged path before atomically publishing.
    ///
    /// The verifier runs after generic header/payload checks and before
    /// persist. On any failure the staged temporary is discarded and the final
    /// path is left untouched (Deny never clobbers; Replace never deletes the
    /// old file before a successful rename).
    pub fn write_atomic_verified<F>(
        &self,
        output: &Path,
        overwrite: OverwritePolicy,
        verify: F,
    ) -> Result<WriteReport>
    where
        F: FnOnce(&Path) -> Result<()>,
    {
        self.write_atomic_verified_before_publish(output, overwrite, verify, || Ok(()))
    }

    /// Write and verify a staged MPKG, then run a final safety hook immediately
    /// before the atomic persist operation.
    pub(crate) fn write_atomic_verified_before_publish<F, G>(
        &self,
        output: &Path,
        overwrite: OverwritePolicy,
        verify: F,
        before_publish: G,
    ) -> Result<WriteReport>
    where
        F: FnOnce(&Path) -> Result<()>,
        G: FnOnce() -> Result<()>,
    {
        if !self.version.is_writable() {
            return Err(Error::InvalidArguments {
                reason: format!(
                    "writer only supports PKGM0018/PKGM0020, got {}",
                    self.version.as_magic()
                ),
            });
        }
        if overwrite == OverwritePolicy::Deny && output.exists() {
            return Err(pack_io(
                output,
                io::Error::new(io::ErrorKind::AlreadyExists, "output already exists"),
            ));
        }
        let layout = self.checked_layout()?;
        self.verify_file_sources()?;
        let parent = output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if output.file_name().is_none() {
            return Err(Error::InvalidArguments {
                reason: format!("output must name a file: {}", output.display()),
            });
        }

        let mut temp = tempfile::Builder::new()
            .prefix(".pkg2mpkg-")
            .suffix(".partial")
            .tempfile_in(parent)
            .map_err(|source| pack_io(parent, source))?;
        {
            let mut writer = BufWriter::new(temp.as_file_mut());
            self.write_header_and_table(&mut writer, &layout, output)?;
            self.write_payloads(&mut writer, output)?;
            writer.flush().map_err(|source| pack_io(output, source))?;
        }
        temp.as_file()
            .sync_all()
            .map_err(|source| pack_io(output, source))?;
        self.verify_temporary_package(temp.path(), &layout)?;
        verify(temp.path())?;
        before_publish()?;

        let _persisted = match overwrite {
            OverwritePolicy::Deny => temp.persist_noclobber(output),
            OverwritePolicy::Replace => temp.persist(output),
        }
        .map_err(|error| pack_io(output, error.error))?;

        Ok(WriteReport {
            output: output.to_path_buf(),
            version: self.version,
            entries: self.entries.len(),
            bytes: layout.total_size,
        })
    }

    fn validate_new_path(&self, path: &str) -> Result<String> {
        let archive_path = validate_archive_path(path)?;
        if archive_path.len() > MAX_PATH_LENGTH {
            return Err(Error::invalid_mpkg(format!(
                "archive path exceeds {MAX_PATH_LENGTH} bytes"
            )));
        }
        if self.paths.contains_key(&archive_path) {
            return Err(Error::invalid_mpkg(format!(
                "duplicate archive path: {archive_path}"
            )));
        }
        Ok(archive_path)
    }

    fn checked_layout(&self) -> Result<Layout> {
        let entry_count =
            u32::try_from(self.entries.len()).map_err(|_| Error::PackageTooLarge {
                size: self.entries.len() as u64,
            })?;
        let mut table_size = 0_u64;
        let mut payload_size = 0_u64;
        let mut offsets = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            let path_size = u64::try_from(entry.archive_path.len())
                .map_err(|_| Error::PackageTooLarge { size: u64::MAX })?;
            table_size = table_size
                .checked_add(4)
                .and_then(|value| value.checked_add(path_size))
                .and_then(|value| value.checked_add(4 + 4))
                .ok_or(Error::PackageTooLarge { size: u64::MAX })?;
            let size = entry.source.size();
            offsets.push((payload_size, size));
            payload_size = payload_size
                .checked_add(size)
                .ok_or(Error::PackageTooLarge { size: u64::MAX })?;
        }
        let total_size = HEADER_SIZE
            .checked_add(table_size)
            .and_then(|value| value.checked_add(payload_size))
            .ok_or(Error::PackageTooLarge { size: u64::MAX })?;
        if total_size >= FOUR_GIB {
            return Err(Error::PackageTooLarge { size: total_size });
        }
        for (offset, size) in &offsets {
            u32::try_from(*offset).map_err(|_| Error::PackageTooLarge { size: total_size })?;
            u32::try_from(*size).map_err(|_| Error::PackageTooLarge { size: total_size })?;
        }
        Ok(Layout {
            entry_count,
            offsets,
            total_size,
        })
    }

    fn verify_file_sources(&self) -> Result<()> {
        for entry in &self.entries {
            if let EntrySource::File { path, size } = &entry.source {
                let metadata = fs::metadata(path).map_err(|source| pack_io(path, source))?;
                if !metadata.is_file() || metadata.len() != *size {
                    return Err(Error::ConversionFailed {
                        reason: format!(
                            "entry source changed after it was added: {}",
                            path.display()
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    fn write_header_and_table(
        &self,
        writer: &mut impl Write,
        layout: &Layout,
        output: &Path,
    ) -> Result<()> {
        write_u32(writer, 8, output)?;
        writer
            .write_all(self.version.as_magic().as_bytes())
            .map_err(|source| pack_io(output, source))?;
        write_u32(writer, layout.entry_count, output)?;
        for (entry, (offset, size)) in self.entries.iter().zip(&layout.offsets) {
            let path_length =
                u32::try_from(entry.archive_path.len()).map_err(|_| Error::PackageTooLarge {
                    size: layout.total_size,
                })?;
            write_u32(writer, path_length, output)?;
            writer
                .write_all(entry.archive_path.as_bytes())
                .map_err(|source| pack_io(output, source))?;
            write_u32(
                writer,
                u32::try_from(*offset).map_err(|_| Error::PackageTooLarge {
                    size: layout.total_size,
                })?,
                output,
            )?;
            write_u32(
                writer,
                u32::try_from(*size).map_err(|_| Error::PackageTooLarge {
                    size: layout.total_size,
                })?,
                output,
            )?;
        }
        Ok(())
    }

    fn write_payloads(&self, writer: &mut impl Write, output: &Path) -> Result<()> {
        for entry in &self.entries {
            match &entry.source {
                EntrySource::Bytes(bytes) => writer
                    .write_all(bytes)
                    .map_err(|source| pack_io(output, source))?,
                EntrySource::File { path, size } => {
                    let mut source_file =
                        File::open(path).map_err(|source| pack_io(path, source))?;
                    let copied = {
                        let mut limited = Read::by_ref(&mut source_file).take(*size);
                        io::copy(&mut limited, writer).map_err(|source| pack_io(output, source))?
                    };
                    if copied != *size {
                        return Err(Error::ConversionFailed {
                            reason: format!(
                                "entry source became shorter while reading: {}",
                                path.display()
                            ),
                        });
                    }
                    let mut extra = [0_u8; 1];
                    if source_file
                        .read(&mut extra)
                        .map_err(|source| pack_io(path, source))?
                        != 0
                    {
                        return Err(Error::ConversionFailed {
                            reason: format!(
                                "entry source became longer while reading: {}",
                                path.display()
                            ),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn verify_temporary_package(&self, path: &Path, layout: &Layout) -> Result<()> {
        let archive = MpkgArchive::open(path)?;
        if archive.version() != self.version || archive.entries().len() != self.entries.len() {
            return Err(Error::VerificationFailed {
                reason: "temporary package header did not round-trip".into(),
            });
        }
        for ((expected, (_, size)), actual) in self
            .entries
            .iter()
            .zip(&layout.offsets)
            .zip(archive.entries())
        {
            if actual.path != expected.archive_path || actual.size != *size {
                return Err(Error::VerificationFailed {
                    reason: format!("temporary package entry mismatch: {}", actual.path),
                });
            }
            archive.verify_entry_payload(&actual.path)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct Layout {
    entry_count: u32,
    offsets: Vec<(u64, u64)>,
    total_size: u64,
}

fn write_u32(writer: &mut impl Write, value: u32, output: &Path) -> Result<()> {
    writer
        .write_all(&value.to_le_bytes())
        .map_err(|source| pack_io(output, source))
}

fn pack_io(path: &Path, source: io::Error) -> Error {
    Error::Io {
        stage: Stage::Pack,
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, fs};

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn before_publish_hook_runs_after_staged_verify_and_can_abort_persist() {
        let dir = tempdir().unwrap();
        let output = dir.path().join("protected.mpkg");
        fs::write(&output, b"precious-existing-output").unwrap();
        let mut builder = MpkgBuilder::new(ContainerVersion::Pkgm0020);
        builder
            .add_bytes("project.json", br#"{"type":"scene"}"#.to_vec())
            .unwrap();
        let staged_verified = Cell::new(false);
        let hook_ran = Cell::new(false);

        let error = builder
            .write_atomic_verified_before_publish(
                &output,
                OverwritePolicy::Replace,
                |_| {
                    staged_verified.set(true);
                    Ok(())
                },
                || {
                    assert!(staged_verified.get(), "hook ran before staged verifier");
                    hook_ran.set(true);
                    Err(Error::InvalidArguments {
                        reason: "test safety hook rejected publish".into(),
                    })
                },
            )
            .unwrap_err();

        assert!(matches!(error, Error::InvalidArguments { .. }));
        assert!(hook_ran.get());
        assert_eq!(fs::read(&output).unwrap(), b"precious-existing-output");
        assert!(
            fs::read_dir(dir.path())
                .unwrap()
                .filter_map(std::result::Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().contains("partial"))
        );
    }
}
