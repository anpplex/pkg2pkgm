use serde_json::{Map, Value};

use crate::{
    Error, ExportPlan, Result, SourceProject, WallpaperKind, mpkg::path::normalize_archive_path,
};

/// Build a mobile `project.json` by cloning the inspected source and replacing
/// only `/general/properties` with the plan's sanitized override.
///
/// Output is deterministic: every object key is sorted recursively, serialization
/// is compact UTF-8 JSON, and exactly one trailing LF is appended.
pub fn build_mobile_scene_project_json(
    source: &SourceProject,
    plan: &ExportPlan,
) -> Result<Vec<u8>> {
    if source.kind != WallpaperKind::Scene {
        return Err(Error::InvalidProject {
            reason: format!(
                "mobile scene project.json requires a Scene source, got {}",
                source.kind.as_str()
            ),
        });
    }

    revalidate_scene_entry(source)?;

    let mut root = source.manifest.raw().clone();
    if !root.is_object() {
        return Err(Error::InvalidProject {
            reason: "project.json root must be an object".into(),
        });
    }

    match root.get("type") {
        None | Some(Value::Null) => {}
        Some(Value::String(kind)) if kind.eq_ignore_ascii_case("scene") => {}
        Some(Value::String(kind)) => {
            return Err(Error::InvalidProject {
                reason: format!(
                    "mobile scene project.json cannot change type away from scene: {kind}"
                ),
            });
        }
        Some(_) => {
            return Err(Error::InvalidProject {
                reason: "project.json type must be a string".into(),
            });
        }
    }

    let properties = plan_properties_object(plan);
    set_general_properties(&mut root, properties)?;

    sort_value_keys(&mut root);

    let mut bytes = serde_json::to_vec(&root).map_err(|source_err| Error::InvalidProject {
        reason: format!("failed to serialize mobile project.json: {source_err}"),
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn revalidate_scene_entry(source: &SourceProject) -> Result<()> {
    let entry = source
        .manifest
        .entry()
        .ok_or_else(|| Error::InvalidProject {
            reason: "project.json is missing a string file field".into(),
        })?;
    let entry = normalize_archive_path(entry).map_err(|path| Error::InvalidProject {
        reason: format!("unsafe scene entry path: {path:?}"),
    })?;

    let entry_path = source.root.join(&entry);
    let metadata = std::fs::symlink_metadata(&entry_path).map_err(|source_err| {
        if source_err.kind() == std::io::ErrorKind::NotFound {
            Error::InvalidProject {
                reason: format!("scene entry is missing: {entry}"),
            }
        } else {
            Error::Io {
                stage: crate::Stage::Inspect,
                path: entry_path.clone(),
                source: source_err,
            }
        }
    })?;
    if metadata.file_type().is_symlink() {
        return Err(Error::InvalidProject {
            reason: format!("scene entry must not be a symlink: {entry}"),
        });
    }
    if !metadata.is_file() {
        return Err(Error::InvalidProject {
            reason: format!("scene entry is not a regular file: {entry}"),
        });
    }
    Ok(())
}

fn plan_properties_object(plan: &ExportPlan) -> Map<String, Value> {
    plan.properties
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn set_general_properties(root: &mut Value, properties: Map<String, Value>) -> Result<()> {
    let object = root.as_object_mut().expect("root checked as object");

    match object.get("general") {
        None | Some(Value::Null) => {
            object.insert(
                "general".into(),
                Value::Object(Map::from_iter([(
                    "properties".into(),
                    Value::Object(properties),
                )])),
            );
            Ok(())
        }
        Some(Value::Object(_)) => {
            let general = object
                .get_mut("general")
                .and_then(Value::as_object_mut)
                .expect("general is object");
            match general.get("properties") {
                None | Some(Value::Null) | Some(Value::Object(_)) => {
                    general.insert("properties".into(), Value::Object(properties));
                    Ok(())
                }
                Some(_) => Err(Error::InvalidProject {
                    reason: "project.json /general/properties must be an object".into(),
                }),
            }
        }
        Some(_) => Err(Error::InvalidProject {
            reason: "project.json /general must be an object".into(),
        }),
    }
}

fn sort_value_keys(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = std::mem::take(map).into_iter().collect();
            entries.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
            for (_, child) in &mut entries {
                sort_value_keys(child);
            }
            *map = entries.into_iter().collect();
        }
        Value::Array(items) => {
            for item in items {
                sort_value_keys(item);
            }
        }
        _ => {}
    }
}
