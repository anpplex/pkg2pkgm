use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Error, Result, SceneSourceLimits, SceneSourceTree, SourceProject, Stage, WallpaperKind,
    inventory_scene_source,
    mpkg::{DesktopPackageArchive, path::normalize_archive_path},
    project::ProjectManifest,
};

/// Hard limits applied while unpacking a desktop `scene.pkg`.
///
/// Path rules mirror MPKG: relative `/` form, no `..`, max path length 16384.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenePackageUnpackLimits {
    pub max_entries: u32,
    pub max_path_length: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
}

impl ScenePackageUnpackLimits {
    /// Defaults aligned with MPKG reader ceilings (entries / path length).
    pub const fn mpkg_aligned() -> Self {
        Self {
            max_entries: 1_000_000,
            max_path_length: 16_384,
            max_file_bytes: u64::MAX,
            max_total_bytes: u64::MAX,
        }
    }
}

/// Request to extract a packaged Scene into a task-owned directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenePackageUnpackRequest {
    pub package: PathBuf,
    pub output_dir: PathBuf,
    pub limits: ScenePackageUnpackLimits,
}

impl ScenePackageUnpackRequest {
    pub fn new(
        package: PathBuf,
        output_dir: PathBuf,
        limits: ScenePackageUnpackLimits,
    ) -> Result<Self> {
        if package.as_os_str().is_empty() || output_dir.as_os_str().is_empty() {
            return Err(Error::InvalidArguments {
                reason: "scene package and output directory paths must not be empty".into(),
            });
        }
        if package == output_dir {
            return Err(Error::InvalidArguments {
                reason: format!(
                    "scene package and output directory must differ: {}",
                    package.display()
                ),
            });
        }
        if limits.max_entries == 0
            || limits.max_path_length == 0
            || limits.max_file_bytes == 0
            || limits.max_total_bytes == 0
        {
            return Err(Error::InvalidArguments {
                reason: "scene package unpack limits must be non-zero".into(),
            });
        }
        Ok(Self {
            package,
            output_dir,
            limits,
        })
    }
}

/// One file extracted (or listed) from a desktop scene package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenePackageEntry {
    pub path: String,
    pub size: u64,
}

/// Report returned by a successful package unpack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenePackageUnpackReport {
    pub output_dir: PathBuf,
    pub entries: Vec<ScenePackageEntry>,
    pub total_bytes: u64,
}

/// Object-safe boundary for listing/extracting proprietary desktop `scene.pkg`.
///
/// Production uses [`crate::NativeScenePackageUnpackBackend`] (native PKGV);
/// tests may inject fakes.
pub trait ScenePackageUnpackBackend: Send + Sync {
    fn unpack_scene_package(
        &self,
        request: &ScenePackageUnpackRequest,
    ) -> Result<ScenePackageUnpackReport>;
}

/// True when `entry` is a raw desktop package path (case-insensitive `.pkg`).
pub fn package_entry_is_raw_scene_pkg(entry: &str) -> bool {
    let bytes = entry.as_bytes();
    if bytes.len() < 4 {
        return false;
    }
    let ext = &bytes[bytes.len() - 4..];
    ext.eq_ignore_ascii_case(b".pkg")
}

/// Validate backend output against MPKG-aligned path/size policy and the
/// invariant that conversion must not pretend success with only raw `.pkg`.
pub fn unpack_scene_package_checked(
    backend: &dyn ScenePackageUnpackBackend,
    request: &ScenePackageUnpackRequest,
) -> Result<ScenePackageUnpackReport> {
    let report = backend.unpack_scene_package(request)?;

    if report.output_dir != request.output_dir {
        return Err(Error::ConversionFailed {
            reason: format!(
                "scene package backend reported output {} instead of {}",
                report.output_dir.display(),
                request.output_dir.display()
            ),
        });
    }

    if report.entries.len() as u32 > request.limits.max_entries {
        return Err(Error::InvalidProject {
            reason: format!(
                "unpacked entry count {} exceeds limit {}",
                report.entries.len(),
                request.limits.max_entries
            ),
        });
    }

    let mut seen = std::collections::BTreeSet::new();
    let mut total = 0_u64;
    let mut has_non_pkg = false;

    for entry in &report.entries {
        if entry.path.len() > request.limits.max_path_length {
            return Err(Error::InvalidProject {
                reason: format!(
                    "entry path length {} exceeds {}",
                    entry.path.len(),
                    request.limits.max_path_length
                ),
            });
        }
        let normalized =
            normalize_archive_path(&entry.path).map_err(|path| Error::InvalidProject {
                reason: format!("unsafe unpack archive path: {path:?}"),
            })?;
        if normalized != entry.path {
            return Err(Error::InvalidProject {
                reason: format!("unpack path was not normalized: {:?}", entry.path),
            });
        }
        if !seen.insert(entry.path.clone()) {
            return Err(Error::InvalidProject {
                reason: format!("duplicate unpack path: {}", entry.path),
            });
        }
        if entry.size > request.limits.max_file_bytes {
            return Err(Error::InvalidProject {
                reason: format!(
                    "file size {} exceeds per-file limit {} for {}",
                    entry.size, request.limits.max_file_bytes, entry.path
                ),
            });
        }
        total = total
            .checked_add(entry.size)
            .ok_or_else(|| Error::InvalidProject {
                reason: format!("total byte count overflow while validating {}", entry.path),
            })?;
        if total > request.limits.max_total_bytes {
            return Err(Error::InvalidProject {
                reason: format!(
                    "total size {total} exceeds total limit {} while validating {}",
                    request.limits.max_total_bytes, entry.path
                ),
            });
        }

        if !package_entry_is_raw_scene_pkg(&entry.path) {
            has_non_pkg = true;
        }

        // If the backend materialised files, enforce no-symlink / regular-file.
        let dest = join_archive_path(&report.output_dir, &entry.path);
        if dest.exists() || dest.symlink_metadata().is_ok() {
            let meta = fs::symlink_metadata(&dest).map_err(|source| Error::Io {
                stage: Stage::Unpack,
                path: dest.clone(),
                source,
            })?;
            if meta.file_type().is_symlink() {
                return Err(Error::InvalidProject {
                    reason: format!(
                        "symlink is not allowed in unpacked scene package: {}",
                        dest.display()
                    ),
                });
            }
            if !meta.is_file() {
                return Err(Error::InvalidProject {
                    reason: format!("unpacked entry is not a regular file: {}", dest.display()),
                });
            }
        }
    }

    if report.total_bytes != total {
        return Err(Error::ConversionFailed {
            reason: format!(
                "scene package backend reported total_bytes {} but entry sizes sum to {total}",
                report.total_bytes
            ),
        });
    }

    // Never claim success while the unpack product is still only raw .pkg.
    if report.entries.is_empty()
        || report
            .entries
            .iter()
            .all(|entry| package_entry_is_raw_scene_pkg(&entry.path))
        || !has_non_pkg
    {
        return Err(Error::ConversionFailed {
            reason: "scene package unpack must materialize loose scene files; raw scene.pkg alone is not a successful Android conversion".into(),
        });
    }

    Ok(report)
}

/// Fail if a source still uses a raw `.pkg` as its Scene entry.
pub fn validate_unpacked_scene_tree(source: &SourceProject) -> Result<()> {
    let entry = source
        .manifest
        .entry()
        .ok_or_else(|| Error::InvalidProject {
            reason: "project.json is missing a string file field".into(),
        })?;
    if package_entry_is_raw_scene_pkg(entry) {
        return Err(Error::ConversionFailed {
            reason: format!(
                "raw package entry {entry:?} cannot be used as an Android Scene entry; unpack first"
            ),
        });
    }
    Ok(())
}

/// Result of unpacking a packaged Scene into a task-owned tree and inventorying it.
#[derive(Debug, Clone)]
pub struct PreparedPackagedScene {
    pub source: SourceProject,
    pub tree: SceneSourceTree,
    pub unpack: ScenePackageUnpackReport,
}

/// Unpack `source`'s resolved `.pkg` file into `task_dir`, merge project/preview metadata,
/// rewrite the entry to a loose scene file, inventory, and reject residual `.pkg`.
pub fn prepare_packaged_scene_source(
    source: &SourceProject,
    backend: &dyn ScenePackageUnpackBackend,
    task_dir: &Path,
    unpack_limits: ScenePackageUnpackLimits,
    inventory_limits: SceneSourceLimits,
) -> Result<PreparedPackagedScene> {
    if source.kind != WallpaperKind::Scene {
        return Err(Error::InvalidArguments {
            reason: format!(
                "packaged scene prepare requires a Scene source, got {}",
                source.kind.as_str()
            ),
        });
    }
    let package_path = validated_packaged_scene_file(source)?;
    let request =
        ScenePackageUnpackRequest::new(package_path, task_dir.to_path_buf(), unpack_limits)?;
    let unpack = unpack_scene_package_checked(backend, &request)?;

    // Merge project.json / preview from the original root into the task tree.
    copy_regular_file(
        &source.root.join("project.json"),
        &task_dir.join("project.json"),
    )?;
    if let Some(preview) = source.manifest.raw().get("preview").and_then(Value::as_str) {
        // Normalize before any join/copy so `..` / absolute forms cannot escape task_dir.
        let preview = normalize_archive_path(preview).map_err(|path| Error::InvalidProject {
            reason: format!("unsafe preview path: {path:?}"),
        })?;
        let preview_src = join_archive_path(&source.root, &preview);
        if preview_src.is_file() {
            let dest = join_archive_path(task_dir, &preview);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|io_source| Error::Io {
                    stage: Stage::Unpack,
                    path: parent.to_path_buf(),
                    source: io_source,
                })?;
            }
            copy_regular_file(&preview_src, &dest)?;
        }
    }

    let scene_entry = choose_loose_scene_entry(task_dir, &unpack)?;
    let mut raw = source.manifest.raw().clone();
    if let Some(object) = raw.as_object_mut() {
        object.insert("file".into(), json!(scene_entry));
    }
    let manifest_bytes =
        serde_json::to_vec_pretty(&raw).map_err(|source_err| Error::InvalidProject {
            reason: format!("failed to rewrite project.json after unpack: {source_err}"),
        })?;
    fs::write(task_dir.join("project.json"), manifest_bytes).map_err(|io_source| Error::Io {
        stage: Stage::Unpack,
        path: task_dir.join("project.json"),
        source: io_source,
    })?;

    // Drop any residual raw packages from the task tree so inventory cannot
    // treat them as Android payload.
    remove_raw_pkg_files(task_dir)?;

    let prepared_source = SourceProject {
        root: task_dir.to_path_buf(),
        project_file: Some(task_dir.join("project.json")),
        entry_file: join_archive_path(task_dir, &scene_entry),
        title: source.title.clone(),
        kind: WallpaperKind::Scene,
        manifest: ProjectManifest::parse(&fs::read(task_dir.join("project.json")).map_err(
            |io_source| Error::Io {
                stage: Stage::Unpack,
                path: task_dir.join("project.json"),
                source: io_source,
            },
        )?)?,
    };

    validate_unpacked_scene_tree(&prepared_source)?;
    let tree = inventory_scene_source(&prepared_source, inventory_limits)?;

    if tree
        .entries
        .iter()
        .any(|entry| package_entry_is_raw_scene_pkg(&entry.archive_path))
    {
        return Err(Error::ConversionFailed {
            reason: "prepared scene inventory still contains a raw .pkg entry".into(),
        });
    }

    Ok(PreparedPackagedScene {
        source: prepared_source,
        tree,
        unpack,
    })
}

fn validated_packaged_scene_file(source: &SourceProject) -> Result<PathBuf> {
    if !source
        .entry_file
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pkg"))
    {
        return Err(Error::InvalidArguments {
            reason: format!(
                "source entry is not a packaged scene: {}",
                source.entry_file.display()
            ),
        });
    }

    let canonical_root = fs::canonicalize(&source.root).map_err(|io_source| Error::Io {
        stage: Stage::Unpack,
        path: source.root.clone(),
        source: io_source,
    })?;
    let canonical_package =
        fs::canonicalize(&source.entry_file).map_err(|io_source| Error::Io {
            stage: Stage::Unpack,
            path: source.entry_file.clone(),
            source: io_source,
        })?;
    if !canonical_package.starts_with(&canonical_root) {
        return Err(Error::InvalidProject {
            reason: "packaged Scene entry resolves outside the project root".into(),
        });
    }
    if !canonical_package.is_file() {
        return Err(Error::InvalidProject {
            reason: format!(
                "packaged Scene entry is not a file: {}",
                source.entry_file.display()
            ),
        });
    }

    Ok(source.entry_file.clone())
}

fn choose_loose_scene_entry(
    unpack_root: &Path,
    unpack: &ScenePackageUnpackReport,
) -> Result<String> {
    choose_scene_json_entry(
        unpack.entries.iter().map(|entry| entry.path.as_str()),
        |path, require_scene_marker| {
            is_scene_json_document(unpack_root, path, require_scene_marker)
        },
        "unpacked scene package",
    )
}

fn choose_scene_json_entry<'a>(
    paths: impl Iterator<Item = &'a str>,
    mut is_scene_document: impl FnMut(&str, bool) -> Result<bool>,
    subject: &str,
) -> Result<String> {
    let paths = paths.collect::<Vec<_>>();
    // A canonical, structurally valid scene.json remains authoritative even if
    // the package also contains other top-level JSON assets.
    if paths.contains(&"scene.json") {
        if is_scene_document("scene.json", false)? {
            return Ok("scene.json".into());
        }
        return Err(Error::ConversionFailed {
            reason: "invalid canonical scene.json entry: expected a scene JSON document".into(),
        });
    }

    let mut candidates = Vec::new();
    for path in paths {
        if !is_loose_scene_json_candidate(path) || !is_scene_document(path, true)? {
            continue;
        }
        candidates.push(path);
    }
    candidates.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));

    match candidates.as_slice() {
        [entry] => Ok((*entry).to_owned()),
        [] => Err(Error::ConversionFailed {
            reason: format!("{subject} did not contain a loose scene JSON entry"),
        }),
        _ => Err(Error::ConversionFailed {
            reason: format!(
                "{subject} has ambiguous loose scene JSON entries: {}",
                candidates.join(", ")
            ),
        }),
    }
}

pub(crate) fn read_packaged_scene_document(
    source: &SourceProject,
    max_document_bytes: u64,
) -> Result<Vec<u8>> {
    let package_path = validated_packaged_scene_file(source)?;
    let archive = DesktopPackageArchive::open(&package_path)?;
    let scene_entry = choose_scene_json_entry(
        archive.entries().iter().map(|entry| entry.path.as_str()),
        |path, require_scene_marker| {
            validate_packaged_scene_document_size(&archive, path, max_document_bytes)?;
            let bytes = archive.read_entry(path)?;
            Ok(is_scene_json_bytes(&bytes, require_scene_marker))
        },
        "packaged scene",
    )?;
    validate_packaged_scene_document_size(&archive, &scene_entry, max_document_bytes)?;
    archive.read_entry(&scene_entry)
}

fn validate_packaged_scene_document_size(
    archive: &DesktopPackageArchive,
    path: &str,
    max_document_bytes: u64,
) -> Result<()> {
    let Some(entry) = archive.entries().iter().find(|entry| entry.path == path) else {
        return Err(Error::InvalidProject {
            reason: format!("desktop package entry not found: {path}"),
        });
    };
    if entry.size > max_document_bytes {
        return Err(Error::InvalidProject {
            reason: format!(
                "scene document size {} exceeds {} bytes: {}",
                entry.size, max_document_bytes, path
            ),
        });
    }
    Ok(())
}

fn is_loose_scene_json_candidate(path: &str) -> bool {
    if path.contains('/') || path.eq_ignore_ascii_case("project.json") {
        return false;
    }
    if path
        .as_bytes()
        .get(..".pkg2mpkg-".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b".pkg2mpkg-"))
    {
        return false;
    }
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

fn is_scene_json_document(
    unpack_root: &Path,
    archive_path: &str,
    require_scene_marker: bool,
) -> Result<bool> {
    let path = join_archive_path(unpack_root, archive_path);
    let bytes = fs::read(&path).map_err(|source| Error::Io {
        stage: Stage::Unpack,
        path,
        source,
    })?;
    Ok(is_scene_json_bytes(&bytes, require_scene_marker))
}

fn is_scene_json_bytes(bytes: &[u8], require_scene_marker: bool) -> bool {
    let Ok(Value::Object(root)) = serde_json::from_slice::<Value>(bytes) else {
        return false;
    };
    root.get("objects").is_some_and(Value::is_array)
        && (!require_scene_marker
            || root.get("camera").is_some_and(Value::is_object)
            || root.get("general").is_some_and(Value::is_object))
}

fn remove_raw_pkg_files(root: &Path) -> Result<()> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read = fs::read_dir(&dir).map_err(|source| Error::Io {
            stage: Stage::Unpack,
            path: dir.clone(),
            source,
        })?;
        for item in read {
            let item = item.map_err(|source| Error::Io {
                stage: Stage::Unpack,
                path: dir.clone(),
                source,
            })?;
            let path = item.path();
            let meta = fs::symlink_metadata(&path).map_err(|source| Error::Io {
                stage: Stage::Unpack,
                path: path.clone(),
                source,
            })?;
            if meta.file_type().is_symlink() {
                return Err(Error::InvalidProject {
                    reason: format!(
                        "symlink is not allowed in unpacked scene tree: {}",
                        path.display()
                    ),
                });
            }
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if meta.is_file() {
                let name = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("");
                if package_entry_is_raw_scene_pkg(name)
                    || path
                        .extension()
                        .and_then(|value| value.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("pkg"))
                {
                    fs::remove_file(&path).map_err(|source| Error::Io {
                        stage: Stage::Unpack,
                        path: path.clone(),
                        source,
                    })?;
                }
            }
        }
    }
    Ok(())
}

fn copy_regular_file(from: &Path, to: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(from).map_err(|source| Error::Io {
        stage: Stage::Unpack,
        path: from.to_path_buf(),
        source,
    })?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(Error::InvalidProject {
            reason: format!(
                "expected a regular file to copy into task tree: {}",
                from.display()
            ),
        });
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|source| Error::Io {
            stage: Stage::Unpack,
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::copy(from, to).map_err(|source| Error::Io {
        stage: Stage::Unpack,
        path: to.to_path_buf(),
        source,
    })?;
    Ok(())
}

fn join_archive_path(root: &Path, archive_path: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in archive_path.split('/') {
        path.push(part);
    }
    path
}
