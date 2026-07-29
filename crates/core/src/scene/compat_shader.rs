use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use serde_json::Value;

use crate::{Error, Result, SourceProject, Stage};

/// Parsed `config.json` under `assets/zcompat/scene/shaders/<project-id>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatShaderConfig {
    pub maximum_project_id: u64,
    pub frag: Option<String>,
    pub vert: Option<String>,
}

/// One on-disk zcompat rule directory (folder name = workshop project id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatShaderRule {
    pub project_id: String,
    pub config: CompatShaderConfig,
    pub rule_dir: PathBuf,
}

/// Report of shader files replaced in a scene project tree.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompatShaderApplyReport {
    /// Relative archive paths (forward slash) that were overwritten.
    pub replaced: Vec<String>,
}

/// Parse a zcompat `config.json` body.
pub fn parse_compat_shader_config(bytes: &[u8]) -> Result<CompatShaderConfig> {
    let value: Value = serde_json::from_slice(bytes).map_err(|source| Error::InvalidProject {
        reason: format!("zcompat config.json is not valid JSON: {source}"),
    })?;
    let object = value.as_object().ok_or_else(|| Error::InvalidProject {
        reason: "zcompat config.json root must be an object".into(),
    })?;

    let maximum_raw = object
        .get("maximumprojectid")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidProject {
            reason: "zcompat config.json is missing string maximumprojectid".into(),
        })?;
    let maximum_project_id = maximum_raw
        .parse::<u64>()
        .map_err(|_| Error::InvalidProject {
            reason: format!("zcompat maximumprojectid is not a decimal u64: {maximum_raw:?}"),
        })?;

    let frag = optional_shader_basename(object.get("frag"), "frag")?;
    let vert = optional_shader_basename(object.get("vert"), "vert")?;
    if frag.is_none() && vert.is_none() {
        return Err(Error::InvalidProject {
            reason: "zcompat config.json must name at least one of frag or vert".into(),
        });
    }

    Ok(CompatShaderConfig {
        maximum_project_id,
        frag,
        vert,
    })
}

/// Load a rule from `.../shaders/<project-id>/` (folder name supplies the id).
pub fn load_compat_shader_rule(rule_dir: &Path) -> Result<CompatShaderRule> {
    let project_id = rule_dir
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::InvalidProject {
            reason: format!(
                "zcompat rule directory name is not valid UTF-8: {}",
                rule_dir.display()
            ),
        })?
        .to_owned();
    validate_project_id_token(&project_id)?;

    let config_path = rule_dir.join("config.json");
    let bytes = fs::read(&config_path).map_err(|source| Error::Io {
        stage: Stage::Inspect,
        path: config_path,
        source,
    })?;
    let config = parse_compat_shader_config(&bytes)?;

    Ok(CompatShaderRule {
        project_id,
        config,
        rule_dir: rule_dir.to_path_buf(),
    })
}

impl CompatShaderRule {
    /// Apply only when folder project id equals `project_id` and the numeric
    /// id is at most `maximumprojectid`.
    pub fn applies_to(&self, project_id: &str) -> bool {
        if self.project_id != project_id {
            return false;
        }
        match project_id.parse::<u64>() {
            Ok(id) => id <= self.config.maximum_project_id,
            Err(_) => false,
        }
    }
}

/// Read optional `workshopid` from a project manifest (decimal string).
pub fn workshop_project_id(source: &SourceProject) -> Option<String> {
    source
        .manifest
        .raw()
        .get("workshopid")
        .and_then(|value| match value {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => number.as_u64().map(|id| id.to_string()),
            _ => None,
        })
}

/// Apply matching zcompat shader overrides into `project_root`.
///
/// Missing zcompat trees or non-matching rules leave the project unchanged.
pub fn apply_compat_shaders(
    project_root: &Path,
    project_id: &str,
    zcompat_shaders_root: &Path,
) -> Result<CompatShaderApplyReport> {
    validate_project_id_token(project_id)?;

    let rule_dir = zcompat_shaders_root.join(project_id);
    if !rule_dir.is_dir() {
        return Ok(CompatShaderApplyReport::default());
    }

    let rule = load_compat_shader_rule(&rule_dir)?;
    if !rule.applies_to(project_id) {
        return Ok(CompatShaderApplyReport::default());
    }

    let mut targets: Vec<String> = Vec::new();
    if let Some(frag) = rule.config.frag.as_deref() {
        targets.push(frag.to_owned());
    }
    if let Some(vert) = rule.config.vert.as_deref() {
        targets.push(vert.to_owned());
    }

    // Resolve replacement sources first; reject symlinks before any write.
    let mut replacements: Vec<(String, PathBuf, Vec<u8>)> = Vec::new();
    for basename in &targets {
        let source_path = rule.rule_dir.join(basename);
        let meta = fs::symlink_metadata(&source_path).map_err(|source| Error::Io {
            stage: Stage::Inspect,
            path: source_path.clone(),
            source,
        })?;
        if meta.file_type().is_symlink() {
            return Err(Error::InvalidProject {
                reason: format!(
                    "symlink is not allowed as zcompat shader source: {}",
                    source_path.display()
                ),
            });
        }
        if !meta.is_file() {
            return Err(Error::InvalidProject {
                reason: format!(
                    "zcompat shader source is not a regular file: {}",
                    source_path.display()
                ),
            });
        }
        let bytes = fs::read(&source_path).map_err(|source| Error::Io {
            stage: Stage::Inspect,
            path: source_path.clone(),
            source,
        })?;
        replacements.push((basename.clone(), source_path, bytes));
    }

    // Preflight every destination target before the first write so a late
    // failure cannot leave a partial shader replacement set on disk.
    let mut planned: Vec<(String, PathBuf, usize)> = Vec::new();
    for (index, (basename, _source_path, _bytes)) in replacements.iter().enumerate() {
        for relative in find_files_with_basename(project_root, basename)? {
            let dest = join_archive_path(project_root, &relative);
            let meta = fs::symlink_metadata(&dest).map_err(|source| Error::Io {
                stage: Stage::Convert,
                path: dest.clone(),
                source,
            })?;
            if meta.file_type().is_symlink() {
                return Err(Error::InvalidProject {
                    reason: format!(
                        "symlink is not allowed as zcompat shader target: {}",
                        dest.display()
                    ),
                });
            }
            if !meta.is_file() {
                return Err(Error::InvalidProject {
                    reason: format!(
                        "zcompat shader target is not a regular file: {}",
                        dest.display()
                    ),
                });
            }
            planned.push((relative, dest, index));
        }
    }

    let mut replaced = Vec::new();
    for (relative, dest, index) in planned {
        let bytes = &replacements[index].2;
        fs::write(&dest, bytes).map_err(|source| Error::Io {
            stage: Stage::Convert,
            path: dest,
            source,
        })?;
        replaced.push(relative);
    }

    replaced.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    replaced.dedup();
    Ok(CompatShaderApplyReport { replaced })
}

fn optional_shader_basename(value: Option<&Value>, field: &str) -> Result<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            validate_shader_basename(text)?;
            Ok(Some(text.clone()))
        }
        Some(_) => Err(Error::InvalidProject {
            reason: format!("zcompat config.json {field} must be a string when present"),
        }),
    }
}

fn validate_shader_basename(name: &str) -> Result<()> {
    if name.is_empty()
        || name.contains('\0')
        || name.chars().any(|ch| ch.is_control())
        || name.contains('/')
        || name.contains('\\')
        || name == "."
        || name == ".."
    {
        return Err(Error::InvalidProject {
            reason: format!("unsafe zcompat shader path: {name:?}"),
        });
    }
    let bytes = name.as_bytes();
    let drive_prefix = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if drive_prefix {
        return Err(Error::InvalidProject {
            reason: format!("unsafe zcompat shader path: {name:?}"),
        });
    }
    // Basename only: Path must be a single Normal component.
    let path = Path::new(name);
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(part)), None) if part.to_str() == Some(name) => Ok(()),
        _ => Err(Error::InvalidProject {
            reason: format!("unsafe zcompat shader path: {name:?}"),
        }),
    }
}

fn validate_project_id_token(project_id: &str) -> Result<()> {
    if project_id.is_empty()
        || project_id.contains('/')
        || project_id.contains('\\')
        || project_id.contains('\0')
        || project_id == "."
        || project_id == ".."
    {
        return Err(Error::InvalidProject {
            reason: format!("unsafe workshop project id: {project_id:?}"),
        });
    }
    if !project_id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::InvalidProject {
            reason: format!("workshop project id must be decimal digits: {project_id:?}"),
        });
    }
    Ok(())
}

fn find_files_with_basename(root: &Path, basename: &str) -> Result<Vec<String>> {
    let mut matches = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read = fs::read_dir(&dir).map_err(|source| Error::Io {
            stage: Stage::Inspect,
            path: dir.clone(),
            source,
        })?;
        for item in read {
            let item = item.map_err(|source| Error::Io {
                stage: Stage::Inspect,
                path: dir.clone(),
                source,
            })?;
            let path = item.path();
            let meta = fs::symlink_metadata(&path).map_err(|source| Error::Io {
                stage: Stage::Inspect,
                path: path.clone(),
                source,
            })?;
            if meta.file_type().is_symlink() {
                // Ignore links during discovery; targets that are links are
                // rejected at write time if basename matched another path.
                continue;
            }
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            if name == basename {
                matches.push(relative_archive_path(root, &path)?);
            }
        }
    }
    matches.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    Ok(matches)
}

fn relative_archive_path(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root).map_err(|_| Error::InvalidProject {
        reason: format!(
            "path escapes project root during zcompat scan: {}",
            path.display()
        ),
    })?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => {
                let text = part.to_str().ok_or_else(|| Error::InvalidProject {
                    reason: format!("non-UTF-8 path component: {}", path.display()),
                })?;
                parts.push(text);
            }
            _ => {
                return Err(Error::InvalidProject {
                    reason: format!(
                        "unsafe path component during zcompat scan: {}",
                        path.display()
                    ),
                });
            }
        }
    }
    if parts.is_empty() {
        return Err(Error::InvalidProject {
            reason: "zcompat scan produced an empty relative path".into(),
        });
    }
    Ok(parts.join("/"))
}

fn join_archive_path(root: &Path, archive_path: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in archive_path.split('/') {
        path.push(part);
    }
    path
}
