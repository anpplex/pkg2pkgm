#![forbid(unsafe_code)]

mod backend;
mod error;
mod execute;
mod export;
mod mpkg;
mod profile;
mod project;
mod property;
mod resolution;
mod scene;

pub use backend::{
    BackendCapabilities, HelperRequirement, ResourceTranscodeBackend, TextureTranscodeReport,
    TextureTranscodeRequest, transcode_texture_checked,
};
pub use error::{Error, ErrorCode, Result, Stage};
pub use execute::{ExportContext, ExportReport, execute_export_plan};
pub use export::{
    CompatibilityTarget, ExportMode, ExportPlan, ExportRequest, RequestedExportMode,
    Transformation, VideoInputCompatibility, build_export_plan,
};
pub use mpkg::{
    ContainerVersion, DesktopPackageArchive, MpkgArchive, MpkgBuilder, MpkgEntry,
    NativeScenePackageUnpackBackend, OverwritePolicy, WriteReport,
};
pub use profile::{
    Compression, ContentClass, Reduction, SceneMode, SceneProfile, classify_content_class,
    resolve_scene_profile,
};
pub use project::{
    ProjectManifest, SourceProject, WallpaperKind, inspect_source, source_requires_package_unpack,
};
pub use property::sanitize_mobile_properties;
pub use resolution::{
    Alignment, CropMode, CropRect, Dimensions, ResolvedVideoGeometry, resolve_video_geometry,
};
pub use scene::{
    CompatShaderApplyReport, CompatShaderConfig, CompatShaderRule, PreparedPackagedScene,
    ScenePackageEntry, ScenePackageUnpackBackend, ScenePackageUnpackLimits,
    ScenePackageUnpackReport, ScenePackageUnpackRequest, SceneReferenceReport, SceneSourceEntry,
    SceneSourceLimits, SceneSourceTree, apply_compat_shaders, build_mobile_scene_project_json,
    inventory_scene_source, load_compat_shader_rule, package_entry_is_raw_scene_pkg,
    parse_compat_shader_config, prepare_packaged_scene_source, unpack_scene_package_checked,
    validate_scene_references, validate_unpacked_scene_tree, workshop_project_id,
};
