use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use serde_json::json;

use crate::{Error, MpkgArchive, Result, Stage};

use super::{ProjectManifest, WallpaperKind};

#[derive(Debug, Clone)]
pub struct SourceProject {
    pub root: PathBuf,
    pub project_file: Option<PathBuf>,
    pub entry_file: PathBuf,
    pub title: String,
    pub kind: WallpaperKind,
    pub manifest: ProjectManifest,
}

/// True when the inspected Scene still uses a desktop `.pkg` entry that must be
/// unpacked before Android packaging (see `prepare_packaged_scene_source`).
pub fn source_requires_package_unpack(source: &SourceProject) -> bool {
    source.kind == WallpaperKind::Scene && has_extension(&source.entry_file, &["pkg"])
}

pub fn inspect_source(path: &Path) -> Result<SourceProject> {
    let metadata = fs::metadata(path).map_err(|source| Error::Io {
        stage: Stage::Inspect,
        path: path.to_path_buf(),
        source,
    })?;

    if metadata.is_file() && has_extension(path, &["mp4", "webm"]) {
        return inspect_direct_video(path);
    }

    if metadata.is_file() && has_extension(path, &["mpkg"]) {
        return inspect_mpkg_package(path);
    }

    let project_file = find_project_file(path, metadata.is_dir())?;
    let project_bytes = fs::read(&project_file).map_err(|source| Error::Io {
        stage: Stage::Inspect,
        path: project_file.clone(),
        source,
    })?;
    let manifest = ProjectManifest::parse(&project_bytes)?;
    let root = project_file
        .parent()
        .ok_or_else(|| Error::InvalidProject {
            reason: "project.json has no parent directory".into(),
        })?
        .to_path_buf();
    let entry = manifest.entry().ok_or_else(|| Error::InvalidProject {
        reason: "project.json is missing a string file field".into(),
    })?;
    validate_project_entry(entry)?;
    let declared_kind = manifest.declared_kind()?;
    let entry_file = resolve_project_entry_file(&root, entry, declared_kind)?;

    let canonical_root = canonicalize_for_inspection(&root)?;
    let canonical_entry = canonicalize_for_inspection(&entry_file)?;
    if !canonical_entry.starts_with(&canonical_root) {
        return Err(Error::InvalidProject {
            reason: "project entry resolves outside the project root".into(),
        });
    }

    let kind = match declared_kind {
        Some(kind) => kind,
        None => infer_kind_from_file(entry, &canonical_entry)?,
    };
    reject_unsupported(kind)?;
    let title = manifest
        .title()
        .map(str::to_owned)
        .or_else(|| {
            entry_file
                .file_stem()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
        })
        .ok_or_else(|| Error::InvalidProject {
            reason: "project title is missing and cannot be inferred".into(),
        })?;

    Ok(SourceProject {
        root,
        project_file: Some(project_file),
        entry_file,
        title,
        kind,
        manifest,
    })
}

fn resolve_project_entry_file(
    root: &Path,
    entry: &str,
    declared_kind: Option<WallpaperKind>,
) -> Result<PathBuf> {
    let entry_file = root.join(entry);
    match fs::symlink_metadata(&entry_file) {
        Ok(metadata) if metadata.is_file() => Ok(entry_file),
        Ok(_) => Err(Error::InvalidProject {
            reason: format!("project entry is not a file: {}", entry_file.display()),
        }),
        Err(source)
            if source.kind() == std::io::ErrorKind::NotFound
                && declared_kind == Some(WallpaperKind::Scene)
                && has_extension(Path::new(entry), &["json"]) =>
        {
            let package_file = root.join(Path::new(entry).with_extension("pkg"));
            match fs::symlink_metadata(&package_file) {
                Ok(metadata) if metadata.is_file() => Ok(package_file),
                Ok(_) => Err(Error::InvalidProject {
                    reason: format!(
                        "packaged Scene fallback is not a file: {}",
                        package_file.display()
                    ),
                }),
                Err(package_source) if package_source.kind() == std::io::ErrorKind::NotFound => {
                    Err(Error::Io {
                        stage: Stage::Inspect,
                        path: entry_file,
                        source,
                    })
                }
                Err(package_source) => Err(Error::Io {
                    stage: Stage::Inspect,
                    path: package_file,
                    source: package_source,
                }),
            }
        }
        Err(source) => Err(Error::Io {
            stage: Stage::Inspect,
            path: entry_file,
            source,
        }),
    }
}

/// Inspect a packed Android MPKG without extracting payloads to disk.
///
/// `root` is the package parent directory; `entry_file` is the package path
/// itself (embedded video/scene content is not required as loose files).
/// `project_file` is `None` because the manifest lives inside the archive.
fn inspect_mpkg_package(path: &Path) -> Result<SourceProject> {
    let archive = MpkgArchive::open(path)?;
    let project_bytes = archive.read_entry("project.json")?;
    let manifest = ProjectManifest::parse(&project_bytes)?;
    let entry = manifest.entry().ok_or_else(|| Error::InvalidProject {
        reason: "project.json is missing a string file field".into(),
    })?;
    validate_project_entry(entry)?;

    // Classify before entry-presence checks so stripped third-party Web packs
    // (project.json + preview only) report UnsupportedWallpaperType / exit 3
    // rather than a confusing "entry not found" InvalidProject.
    let kind = match manifest.declared_kind()? {
        Some(kind) => kind,
        None => WallpaperKind::infer_from_entry(entry).ok_or_else(|| Error::InvalidProject {
            reason: format!("cannot infer wallpaper type from package entry: {entry}"),
        })?,
    };
    reject_unsupported(kind)?;

    if !archive.entries().iter().any(|item| item.path == entry) {
        return Err(Error::InvalidProject {
            reason: format!("package entry not found in archive: {entry}"),
        });
    }

    let title = manifest
        .title()
        .map(str::to_owned)
        .or_else(|| {
            Path::new(entry)
                .file_stem()
                .and_then(|value| value.to_str())
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
        .or_else(|| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
        .ok_or_else(|| Error::InvalidProject {
            reason: "project title is missing and cannot be inferred".into(),
        })?;

    let root = path
        .parent()
        .ok_or_else(|| Error::InvalidProject {
            reason: "MPKG path has no parent directory".into(),
        })?
        .to_path_buf();

    Ok(SourceProject {
        root,
        project_file: None,
        entry_file: path.to_path_buf(),
        title,
        kind,
        manifest,
    })
}

fn inspect_direct_video(path: &Path) -> Result<SourceProject> {
    let entry_file = path.to_path_buf();
    let root = entry_file
        .parent()
        .ok_or_else(|| Error::InvalidProject {
            reason: "video path has no parent directory".into(),
        })?
        .to_path_buf();
    let title = entry_file
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::InvalidProject {
            reason: "video filename is not valid UTF-8".into(),
        })?
        .to_owned();
    let file = entry_file
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| Error::InvalidProject {
            reason: "video filename is not valid UTF-8".into(),
        })?;
    let manifest = ProjectManifest::parse(
        serde_json::to_vec(&json!({
            "title": title,
            "type": "video",
            "file": file,
        }))
        .expect("serializing a fixed JSON object cannot fail")
        .as_slice(),
    )?;

    Ok(SourceProject {
        root,
        project_file: None,
        entry_file,
        title,
        kind: WallpaperKind::Video,
        manifest,
    })
}

fn find_project_file(path: &Path, is_directory: bool) -> Result<PathBuf> {
    if is_directory {
        return require_project_file(path.join("project.json"));
    }
    if path
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("project.json"))
    {
        return require_project_file(path.to_path_buf());
    }
    if has_extension(path, &["pkg"]) {
        let parent = path.parent().ok_or_else(|| Error::InvalidProject {
            reason: "PKG path has no parent directory".into(),
        })?;
        for candidate in [
            Some(parent.join("project.json")),
            parent.parent().map(|value| value.join("project.json")),
        ]
        .into_iter()
        .flatten()
        {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        return Err(Error::InvalidProject {
            reason: format!(
                "could not find project.json beside or above {}",
                path.display()
            ),
        });
    }
    Err(Error::InvalidProject {
        reason: format!("unsupported input path: {}", path.display()),
    })
}

fn require_project_file(path: PathBuf) -> Result<PathBuf> {
    if path.is_file() {
        Ok(path)
    } else {
        Err(Error::InvalidProject {
            reason: format!("project.json not found at {}", path.display()),
        })
    }
}

fn validate_project_entry(entry: &str) -> Result<()> {
    let bytes = entry.as_bytes();
    let drive_prefix = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    let path = Path::new(entry);
    if entry.is_empty()
        || entry.contains('\0')
        || entry.contains('\\')
        || drive_prefix
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(Error::InvalidProject {
            reason: format!("unsafe project entry path: {entry:?}"),
        });
    }
    Ok(())
}

fn infer_kind_from_file(entry: &str, entry_file: &Path) -> Result<WallpaperKind> {
    if let Some(kind) = WallpaperKind::infer_from_entry(entry) {
        return Ok(kind);
    }
    if !has_extension(entry_file, &["json"]) {
        return Err(Error::InvalidProject {
            reason: format!("cannot infer wallpaper type from entry: {entry}"),
        });
    }
    let bytes = fs::read(entry_file).map_err(|source| Error::Io {
        stage: Stage::Inspect,
        path: entry_file.to_path_buf(),
        source,
    })?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|source| Error::InvalidProject {
            reason: format!("Scene entry is not valid JSON: {source}"),
        })?;
    let object = value.as_object().ok_or_else(|| Error::InvalidProject {
        reason: "Scene entry JSON root must be an object".into(),
    })?;
    if ["camera", "objects", "general"]
        .into_iter()
        .any(|marker| object.contains_key(marker))
    {
        Ok(WallpaperKind::Scene)
    } else {
        Err(Error::InvalidProject {
            reason: "JSON entry has no recognized Scene root marker".into(),
        })
    }
}

fn reject_unsupported(kind: WallpaperKind) -> Result<()> {
    match kind {
        WallpaperKind::Scene | WallpaperKind::Video => Ok(()),
        WallpaperKind::Web | WallpaperKind::Application => {
            Err(Error::unsupported_type(kind.as_str()))
        }
    }
}

fn canonicalize_for_inspection(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).map_err(|source| Error::Io {
        stage: Stage::Inspect,
        path: path.to_path_buf(),
        source,
    })
}

fn has_extension(path: &Path, expected: &[&str]) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| expected.iter().any(|item| value.eq_ignore_ascii_case(item)))
}
