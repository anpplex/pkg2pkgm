use serde::{Deserialize, Serialize};

use crate::{
    Error, Result, SourceProject, Stage, WallpaperKind, project::source_requires_package_unpack,
    scene::read_packaged_scene_document,
};

const PIXEL_ART_AREA_THRESHOLD: u64 = 307_200;
const UHD_AREA_THRESHOLD: u64 = 2_075_520;
const MAX_SCENE_DOCUMENT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compression {
    HighQuality,
    HighPerformance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reduction {
    #[serde(rename = "high_quality")]
    Original,
    #[serde(rename = "reduction_x2")]
    X2,
    #[serde(rename = "reduction_x4")]
    X4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentClass {
    PixelArt,
    Normal,
    Uhd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneProfile {
    High,
    Balanced,
    Performance,
    Custom {
        compression: Compression,
        reduction: Reduction,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SceneMode {
    Dynamic {
        compression: Compression,
        reduction: Reduction,
    },
    PreRendered,
}

pub const fn resolve_scene_profile(profile: SceneProfile, class: ContentClass) -> SceneMode {
    match (profile, class) {
        (
            SceneProfile::Custom {
                compression,
                reduction,
            },
            _,
        ) => SceneMode::Dynamic {
            compression,
            reduction,
        },
        (SceneProfile::Performance, _) => SceneMode::PreRendered,
        (SceneProfile::High, ContentClass::PixelArt) => SceneMode::Dynamic {
            compression: Compression::HighQuality,
            reduction: Reduction::Original,
        },
        (SceneProfile::High, ContentClass::Normal) => SceneMode::Dynamic {
            compression: Compression::HighPerformance,
            reduction: Reduction::Original,
        },
        (SceneProfile::High, ContentClass::Uhd) => SceneMode::Dynamic {
            compression: Compression::HighPerformance,
            reduction: Reduction::X2,
        },
        (SceneProfile::Balanced, ContentClass::PixelArt) => SceneMode::Dynamic {
            compression: Compression::HighPerformance,
            reduction: Reduction::Original,
        },
        (SceneProfile::Balanced, ContentClass::Normal) => SceneMode::Dynamic {
            compression: Compression::HighPerformance,
            reduction: Reduction::X2,
        },
        (SceneProfile::Balanced, ContentClass::Uhd) => SceneMode::Dynamic {
            compression: Compression::HighPerformance,
            reduction: Reduction::X4,
        },
    }
}

pub fn classify_content_class(source: &SourceProject) -> Result<ContentClass> {
    if source.kind != WallpaperKind::Scene {
        return Ok(ContentClass::Normal);
    }

    let mut maximum_area = 0_u64;
    let mut has_uhd_tag = false;
    for tag in manifest_tags(source) {
        if let Some((width, height)) = parse_resolution_tag(tag) {
            let area = width
                .checked_mul(height)
                .ok_or_else(|| Error::InvalidProject {
                    reason: format!("resolution tag area overflows: {tag}"),
                })?;
            maximum_area = maximum_area.max(area);
            has_uhd_tag |= area > UHD_AREA_THRESHOLD;
        }
    }
    if has_uhd_tag {
        return Ok(ContentClass::Uhd);
    }

    let scene_bytes = if source
        .entry_file
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("json"))
    {
        let metadata = std::fs::metadata(&source.entry_file).map_err(|source_error| Error::Io {
            stage: Stage::Inspect,
            path: source.entry_file.clone(),
            source: source_error,
        })?;
        if metadata.len() > MAX_SCENE_DOCUMENT_BYTES {
            return Err(Error::InvalidProject {
                reason: format!(
                    "scene document size {} exceeds {} bytes: {}",
                    metadata.len(),
                    MAX_SCENE_DOCUMENT_BYTES,
                    source.entry_file.display()
                ),
            });
        }
        Some(
            std::fs::read(&source.entry_file).map_err(|source_error| Error::Io {
                stage: Stage::Inspect,
                path: source.entry_file.clone(),
                source: source_error,
            })?,
        )
    } else if source_requires_package_unpack(source) {
        Some(read_packaged_scene_document(
            source,
            MAX_SCENE_DOCUMENT_BYTES,
        )?)
    } else {
        None
    };
    if let Some(bytes) = scene_bytes {
        let scene: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|source_error| Error::InvalidProject {
                reason: format!("Scene entry is not valid JSON: {source_error}"),
            })?;
        if let Some((width, height)) = scene_dimensions(&scene) {
            maximum_area = maximum_area.max(width.checked_mul(height).ok_or_else(|| {
                Error::InvalidProject {
                    reason: "Scene projection area overflows".into(),
                }
            })?);
        }
    }

    if maximum_area != 0 && maximum_area < PIXEL_ART_AREA_THRESHOLD {
        Ok(ContentClass::PixelArt)
    } else {
        Ok(ContentClass::Normal)
    }
}

fn manifest_tags(source: &SourceProject) -> Vec<&str> {
    match source.manifest.raw().get("tags") {
        Some(serde_json::Value::Array(tags)) => {
            tags.iter().filter_map(|tag| tag.as_str()).collect()
        }
        Some(serde_json::Value::String(tags)) => tags.split(',').collect(),
        _ => Vec::new(),
    }
}

fn parse_resolution_tag(tag: &str) -> Option<(u64, u64)> {
    let numbers: Vec<_> = tag
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(str::parse::<u64>)
        .collect::<std::result::Result<_, _>>()
        .ok()?;
    if numbers.len() == 2 && tag.to_ascii_lowercase().contains('x') {
        Some((numbers[0], numbers[1]))
    } else {
        None
    }
}

fn scene_dimensions(scene: &serde_json::Value) -> Option<(u64, u64)> {
    let projection = scene.pointer("/general/orthogonalprojection")?;
    let width = json_dimension(projection.get("width")?)?;
    let height = json_dimension(projection.get("height")?)?;
    (width != 0 && height != 0).then_some((width, height))
}

fn json_dimension(value: &serde_json::Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        let number = value.as_f64()?;
        (number.is_finite() && number > 0.0 && number.fract() == 0.0 && number <= u64::MAX as f64)
            .then_some(number as u64)
    })
}
