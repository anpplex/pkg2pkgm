use std::path::PathBuf;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    Compression, ContainerVersion, ContentClass, Error, HelperRequirement, Reduction, Result,
    SceneMode, SceneProfile, SourceProject, WallpaperKind, resolve_scene_profile,
    sanitize_mobile_properties,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case")]
pub enum CompatibilityTarget {
    WeAndroid { major: u8, minor: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoInputCompatibility {
    Unknown,
    AndroidH264Mp4,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequestedExportMode {
    Scene {
        profile: SceneProfile,
        content_class: ContentClass,
    },
    Video {
        input: VideoInputCompatibility,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportRequest {
    pub output: PathBuf,
    pub mode: RequestedExportMode,
}

impl ExportRequest {
    pub fn scene(output: PathBuf, profile: SceneProfile, content_class: ContentClass) -> Self {
        Self {
            output,
            mode: RequestedExportMode::Scene {
                profile,
                content_class,
            },
        }
    }

    pub fn video(output: PathBuf, input: VideoInputCompatibility) -> Self {
        Self {
            output,
            mode: RequestedExportMode::Video { input },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ExportMode {
    SceneDynamic {
        compression: Compression,
        reduction: Reduction,
    },
    ScenePreRenderedVideo,
    Video {
        passthrough: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Transformation {
    SanitizeProperties,
    TranscodeSceneResources {
        compression: Compression,
        reduction: Reduction,
    },
    CaptureScene,
    EncodeH264,
    CopyVideo,
    PackageMpkg {
        version: ContainerVersion,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportPlan {
    pub source: PathBuf,
    pub title: String,
    pub kind: WallpaperKind,
    pub mode: ExportMode,
    pub compatibility: CompatibilityTarget,
    pub properties: IndexMap<String, Value>,
    pub transformations: Vec<Transformation>,
    #[serde(rename = "helper_requirements")]
    pub helpers: Vec<HelperRequirement>,
    pub estimated_size: Option<u64>,
    pub output: PathBuf,
}

pub fn build_export_plan(source: &SourceProject, request: ExportRequest) -> Result<ExportPlan> {
    match source.kind {
        WallpaperKind::Web | WallpaperKind::Application => {
            return Err(Error::unsupported_type(source.kind.as_str()));
        }
        WallpaperKind::Scene | WallpaperKind::Video => {}
    }
    if request.output.as_os_str().is_empty() {
        return Err(Error::InvalidArguments {
            reason: "output path must not be empty".into(),
        });
    }

    let (mode, transformations, helpers) = match (source.kind, request.mode) {
        (
            WallpaperKind::Scene,
            RequestedExportMode::Scene {
                profile,
                content_class,
            },
        ) => scene_plan(profile, content_class),
        (WallpaperKind::Video, RequestedExportMode::Video { input }) => video_plan(input),
        (kind, requested) => {
            return Err(Error::InvalidArguments {
                reason: format!(
                    "request mode {} does not match {} source",
                    request_name(&requested),
                    kind.as_str()
                ),
            });
        }
    };

    let plan_source = match source.kind {
        WallpaperKind::Scene => source.root.clone(),
        WallpaperKind::Video => source.entry_file.clone(),
        WallpaperKind::Web | WallpaperKind::Application => unreachable!("type gate returned above"),
    };
    Ok(ExportPlan {
        source: plan_source,
        title: source.title.clone(),
        kind: source.kind,
        mode,
        compatibility: CompatibilityTarget::WeAndroid { major: 2, minor: 8 },
        properties: sanitized_properties(source),
        transformations,
        helpers,
        estimated_size: None,
        output: request.output,
    })
}

fn scene_plan(
    profile: SceneProfile,
    content_class: ContentClass,
) -> (ExportMode, Vec<Transformation>, Vec<HelperRequirement>) {
    match resolve_scene_profile(profile, content_class) {
        SceneMode::Dynamic {
            compression,
            reduction,
        } => (
            ExportMode::SceneDynamic {
                compression,
                reduction,
            },
            vec![
                Transformation::SanitizeProperties,
                Transformation::TranscodeSceneResources {
                    compression,
                    reduction,
                },
                Transformation::PackageMpkg {
                    version: ContainerVersion::Pkgm0020,
                },
            ],
            vec![HelperRequirement::ResourceTranscode],
        ),
        SceneMode::PreRendered => (
            ExportMode::ScenePreRenderedVideo,
            vec![
                Transformation::SanitizeProperties,
                Transformation::CaptureScene,
                Transformation::EncodeH264,
                Transformation::PackageMpkg {
                    version: ContainerVersion::Pkgm0020,
                },
            ],
            vec![
                HelperRequirement::SceneCapture,
                HelperRequirement::H264Encode,
            ],
        ),
    }
}

fn video_plan(
    input: VideoInputCompatibility,
) -> (ExportMode, Vec<Transformation>, Vec<HelperRequirement>) {
    match input {
        VideoInputCompatibility::Unknown => (
            ExportMode::Video { passthrough: false },
            vec![
                Transformation::SanitizeProperties,
                Transformation::EncodeH264,
                Transformation::PackageMpkg {
                    version: ContainerVersion::Pkgm0020,
                },
            ],
            vec![HelperRequirement::H264Encode],
        ),
        VideoInputCompatibility::AndroidH264Mp4 => (
            ExportMode::Video { passthrough: true },
            vec![
                Transformation::SanitizeProperties,
                Transformation::CopyVideo,
                Transformation::PackageMpkg {
                    version: ContainerVersion::Pkgm0020,
                },
            ],
            Vec::new(),
        ),
    }
}

fn sanitized_properties(source: &SourceProject) -> IndexMap<String, Value> {
    let properties = source
        .manifest
        .raw()
        .pointer("/general/properties")
        .and_then(Value::as_object)
        .or_else(|| {
            source
                .manifest
                .raw()
                .get("properties")
                .and_then(Value::as_object)
        });
    let Some(properties) = properties else {
        return IndexMap::new();
    };
    let mut entries: Vec<_> = sanitize_mobile_properties(properties).into_iter().collect();
    entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    entries.into_iter().collect()
}

fn request_name(mode: &RequestedExportMode) -> &'static str {
    match mode {
        RequestedExportMode::Scene { .. } => "scene",
        RequestedExportMode::Video { .. } => "video",
    }
}
