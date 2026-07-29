use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use crate::{
    Error, Result, SourceProject, Stage, WallpaperKind, mpkg::path::normalize_archive_path,
};

/// Hard limits applied while inventorying a Scene source tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneSourceLimits {
    pub max_files: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
}

/// One regular file selected for later packaging / conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneSourceEntry {
    pub archive_path: String,
    pub source_path: PathBuf,
    pub size: u64,
}

/// Deterministic inventory of regular files under a Scene project root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneSourceTree {
    pub root: PathBuf,
    pub scene_entry: String,
    pub preview: Option<String>,
    pub entries: Vec<SceneSourceEntry>,
    pub total_bytes: u64,
}

/// Walk a Scene source without following links and without reading payload bytes.
pub fn inventory_scene_source(
    source: &SourceProject,
    limits: SceneSourceLimits,
) -> Result<SceneSourceTree> {
    if limits.max_files == 0 || limits.max_file_bytes == 0 || limits.max_total_bytes == 0 {
        return Err(Error::InvalidArguments {
            reason: "scene source limits must be non-zero".into(),
        });
    }
    if source.kind != WallpaperKind::Scene {
        return Err(Error::InvalidArguments {
            reason: format!(
                "scene inventory requires a Scene source, got {}",
                source.kind.as_str()
            ),
        });
    }

    let scene_entry = source
        .manifest
        .entry()
        .ok_or_else(|| Error::InvalidProject {
            reason: "project.json is missing a string file field".into(),
        })?
        .to_owned();
    let scene_entry = normalize_required_archive_path(&scene_entry, "scene entry")?;

    let preview = match source.manifest.raw().get("preview") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(value)) => {
            Some(normalize_required_archive_path(value, "preview")?)
        }
        Some(_) => {
            return Err(Error::InvalidProject {
                reason: "project.json preview must be a string when present".into(),
            });
        }
    };

    let root = source.root.clone();
    let mut entries: Vec<SceneSourceEntry> = Vec::new();
    let mut total_bytes: u64 = 0;
    let mut stack = vec![root.clone()];

    while let Some(dir) = stack.pop() {
        let read = fs::read_dir(&dir).map_err(|source_err| Error::Io {
            stage: Stage::Inspect,
            path: dir.clone(),
            source: source_err,
        })?;

        for item in read {
            let item = item.map_err(|source_err| Error::Io {
                stage: Stage::Inspect,
                path: dir.clone(),
                source: source_err,
            })?;
            let path = item.path();
            let metadata = fs::symlink_metadata(&path).map_err(|source_err| Error::Io {
                stage: Stage::Inspect,
                path: path.clone(),
                source: source_err,
            })?;
            let file_type = metadata.file_type();

            if file_type.is_symlink() {
                return Err(Error::InvalidProject {
                    reason: format!("symlink is not allowed in scene source: {}", path.display()),
                });
            }

            let archive_path = relative_archive_path(&root, &path)?;

            if file_type.is_dir() {
                if is_excluded_archive_path(&archive_path) {
                    continue;
                }
                stack.push(path);
                continue;
            }

            if !file_type.is_file() {
                return Err(Error::InvalidProject {
                    reason: format!(
                        "special file is not allowed in scene source: {}",
                        path.display()
                    ),
                });
            }

            if is_excluded_archive_path(&archive_path) {
                continue;
            }

            let size = metadata.len();
            if size > limits.max_file_bytes {
                return Err(Error::InvalidProject {
                    reason: format!(
                        "file size {size} exceeds per-file limit {} for {archive_path}",
                        limits.max_file_bytes
                    ),
                });
            }

            if entries.len() >= limits.max_files {
                return Err(Error::InvalidProject {
                    reason: format!(
                        "file count exceeds limit {} while inventorying {archive_path}",
                        limits.max_files
                    ),
                });
            }

            let new_total = total_bytes
                .checked_add(size)
                .ok_or_else(|| Error::InvalidProject {
                    reason: format!("total byte count overflow while inventorying {archive_path}"),
                })?;
            if new_total > limits.max_total_bytes {
                return Err(Error::InvalidProject {
                    reason: format!(
                        "total size {new_total} exceeds total limit {} while inventorying {archive_path}",
                        limits.max_total_bytes
                    ),
                });
            }

            total_bytes = new_total;
            entries.push(SceneSourceEntry {
                archive_path,
                source_path: path,
                size,
            });
        }
    }

    entries.sort_by(|a, b| a.archive_path.as_bytes().cmp(b.archive_path.as_bytes()));

    ensure_required_entry(&entries, "project.json", "project.json")?;
    ensure_required_entry(&entries, &scene_entry, "scene entry")?;
    if let Some(preview_path) = preview.as_deref() {
        ensure_required_entry(&entries, preview_path, "preview")?;
    }

    Ok(SceneSourceTree {
        root,
        scene_entry,
        preview,
        entries,
        total_bytes,
    })
}

fn normalize_required_archive_path(path: &str, label: &str) -> Result<String> {
    normalize_archive_path(path).map_err(|path| Error::InvalidProject {
        reason: format!("unsafe {label} path: {path:?}"),
    })
}

fn relative_archive_path(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root).map_err(|_| Error::InvalidProject {
        reason: format!(
            "path escapes project root during inventory: {}",
            path.display()
        ),
    })?;

    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => {
                let text = part.to_str().ok_or_else(|| Error::InvalidProject {
                    reason: format!(
                        "non-UTF-8 path component in scene source: {}",
                        path.display()
                    ),
                })?;
                if text.contains('\\') || text.contains('\0') {
                    return Err(Error::InvalidProject {
                        reason: format!("unsafe path component in scene source: {text:?}"),
                    });
                }
                parts.push(text);
            }
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(Error::InvalidProject {
                    reason: format!("unsafe path component in scene source: {}", path.display()),
                });
            }
        }
    }

    if parts.is_empty() {
        return Err(Error::InvalidProject {
            reason: "inventory produced an empty archive path".into(),
        });
    }

    let joined = parts.join("/");
    normalize_archive_path(&joined).map_err(|path| Error::InvalidProject {
        reason: format!("unsafe archive path: {path:?}"),
    })
}

fn is_excluded_archive_path(archive_path: &str) -> bool {
    for component in archive_path.split('/') {
        if starts_with_ignore_ascii_case(component, ".pkg2mpkg-") {
            return true;
        }
    }
    has_excluded_suffix(archive_path)
}

fn has_excluded_suffix(archive_path: &str) -> bool {
    const SUFFIXES: &[&str] = &[".tex-json", ".mpkg", ".partial"];
    let bytes = archive_path.as_bytes();
    for suffix in SUFFIXES {
        let suffix_bytes = suffix.as_bytes();
        if bytes.len() >= suffix_bytes.len() {
            let tail = &bytes[bytes.len() - suffix_bytes.len()..];
            if tail.eq_ignore_ascii_case(suffix_bytes) {
                return true;
            }
        }
    }
    false
}

fn starts_with_ignore_ascii_case(value: &str, prefix: &str) -> bool {
    value.len() >= prefix.len()
        && value.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

fn ensure_required_entry(
    entries: &[SceneSourceEntry],
    archive_path: &str,
    label: &str,
) -> Result<()> {
    if entries
        .iter()
        .any(|entry| entry.archive_path == archive_path)
    {
        Ok(())
    } else {
        Err(Error::InvalidProject {
            reason: format!("{label} is missing from scene inventory: {archive_path}"),
        })
    }
}
