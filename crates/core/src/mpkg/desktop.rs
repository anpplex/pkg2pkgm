//! Native desktop `scene.pkg` (PKGV) reader and unpack backend.
//!
//! Desktop PKGV and Android MPKG share the same table/payload layout; only the
//! 8-byte magic differs (`PKGV****` vs `PKGM0018`/`PKGM0020`). Format errors
//! surface as [`Error::InvalidProject`] so MPKG's `InvalidMpkg` domain stays
//! reserved for mobile archives.

use std::{
    fs,
    path::{Path, PathBuf},
};

use indexmap::IndexMap;

use crate::{
    Error, Result, Stage,
    scene::{
        ScenePackageEntry, ScenePackageUnpackBackend, ScenePackageUnpackLimits,
        ScenePackageUnpackReport, ScenePackageUnpackRequest,
    },
};

use super::{
    MpkgEntry,
    reader::{
        PackageDirectory, PackageErrorDomain, read_package_directory, read_package_entry_bytes,
    },
};

/// Opened desktop Wallpaper Engine package (`scene.pkg` / PKGV*).
#[derive(Debug)]
pub struct DesktopPackageArchive {
    source: PathBuf,
    magic: String,
    payload_start: u64,
    entries: Vec<MpkgEntry>,
    by_path: IndexMap<String, usize>,
}

impl DesktopPackageArchive {
    /// Open a desktop package. Magic must be exactly 8 bytes starting with `PKGV`.
    pub fn open(path: &Path) -> Result<Self> {
        let domain = PackageErrorDomain::desktop_pkg();
        let directory = read_package_directory(path, &domain)?;
        let magic = validate_desktop_magic(&directory.magic, &domain)?;
        Ok(Self::from_directory(path.to_path_buf(), magic, directory))
    }

    fn from_directory(source: PathBuf, magic: String, directory: PackageDirectory) -> Self {
        Self {
            source,
            magic,
            payload_start: directory.payload_start,
            entries: directory.entries,
            by_path: directory.by_path,
        }
    }

    pub fn magic(&self) -> &str {
        &self.magic
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
            &PackageErrorDomain::desktop_pkg(),
        )
    }

    fn entry_by_path(&self, path: &str) -> Result<&MpkgEntry> {
        let index = self
            .by_path
            .get(path)
            .copied()
            .ok_or_else(|| Error::InvalidProject {
                reason: format!("desktop package entry not found: {path}"),
            })?;
        Ok(&self.entries[index])
    }
}

fn validate_desktop_magic(magic: &[u8; 8], domain: &PackageErrorDomain) -> Result<String> {
    if !magic.starts_with(b"PKGV") {
        let display = String::from_utf8_lossy(magic);
        return Err(domain.format(format!(
            "desktop scene package magic must start with PKGV, got {display:?}"
        )));
    }
    std::str::from_utf8(magic)
        .map(str::to_owned)
        .map_err(|_| domain.format("desktop package magic is not UTF-8"))
}

/// Hermetic native unpack of desktop `scene.pkg` (PKGV) without Wine/device.
#[derive(Debug, Default, Clone, Copy)]
pub struct NativeScenePackageUnpackBackend;

impl NativeScenePackageUnpackBackend {
    pub const fn new() -> Self {
        Self
    }
}

impl ScenePackageUnpackBackend for NativeScenePackageUnpackBackend {
    fn unpack_scene_package(
        &self,
        request: &ScenePackageUnpackRequest,
    ) -> Result<ScenePackageUnpackReport> {
        let archive = DesktopPackageArchive::open(&request.package)?;
        materialize_archive(&archive, request)
    }
}

fn materialize_archive(
    archive: &DesktopPackageArchive,
    request: &ScenePackageUnpackRequest,
) -> Result<ScenePackageUnpackReport> {
    let limits = &request.limits;
    if archive.entries().len() as u32 > limits.max_entries {
        return Err(Error::InvalidProject {
            reason: format!(
                "desktop package entry count {} exceeds limit {}",
                archive.entries().len(),
                limits.max_entries
            ),
        });
    }

    fs::create_dir_all(&request.output_dir).map_err(|source| Error::Io {
        stage: Stage::Unpack,
        path: request.output_dir.clone(),
        source,
    })?;

    let mut report_entries = Vec::with_capacity(archive.entries().len());
    let mut total_bytes = 0_u64;

    for entry in archive.entries() {
        enforce_entry_limits(entry, limits, total_bytes)?;

        let bytes = archive.read_entry(&entry.path)?;
        if bytes.len() as u64 != entry.size {
            return Err(Error::InvalidProject {
                reason: format!(
                    "desktop package entry {} size mismatch: table {} vs payload {}",
                    entry.path,
                    entry.size,
                    bytes.len()
                ),
            });
        }

        total_bytes = total_bytes
            .checked_add(entry.size)
            .ok_or_else(|| Error::InvalidProject {
                reason: format!("total byte count overflow while unpacking {}", entry.path),
            })?;

        let dest = join_archive_path(&request.output_dir, &entry.path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|source| Error::Io {
                stage: Stage::Unpack,
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(&dest, &bytes).map_err(|source| Error::Io {
            stage: Stage::Unpack,
            path: dest.clone(),
            source,
        })?;

        report_entries.push(ScenePackageEntry {
            path: entry.path.clone(),
            size: entry.size,
        });
    }

    Ok(ScenePackageUnpackReport {
        output_dir: request.output_dir.clone(),
        entries: report_entries,
        total_bytes,
    })
}

fn enforce_entry_limits(
    entry: &MpkgEntry,
    limits: &ScenePackageUnpackLimits,
    total_so_far: u64,
) -> Result<()> {
    if entry.path.len() > limits.max_path_length {
        return Err(Error::InvalidProject {
            reason: format!(
                "entry path length {} exceeds {}",
                entry.path.len(),
                limits.max_path_length
            ),
        });
    }
    if entry.size > limits.max_file_bytes {
        return Err(Error::InvalidProject {
            reason: format!(
                "file size {} exceeds per-file limit {} for {}",
                entry.size, limits.max_file_bytes, entry.path
            ),
        });
    }
    let next_total = total_so_far
        .checked_add(entry.size)
        .ok_or_else(|| Error::InvalidProject {
            reason: format!("total byte count overflow while validating {}", entry.path),
        })?;
    if next_total > limits.max_total_bytes {
        return Err(Error::InvalidProject {
            reason: format!(
                "total size {next_total} exceeds total limit {} while validating {}",
                limits.max_total_bytes, entry.path
            ),
        });
    }
    Ok(())
}

fn join_archive_path(root: &Path, archive_path: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in archive_path.split('/') {
        path.push(part);
    }
    path
}
