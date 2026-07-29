mod compat_shader;
mod manifest;
mod mobile_input;
pub(crate) mod package;
mod package_input;
mod references;
mod source_tree;

pub use compat_shader::{
    CompatShaderApplyReport, CompatShaderConfig, CompatShaderRule, apply_compat_shaders,
    load_compat_shader_rule, parse_compat_shader_config, workshop_project_id,
};
pub use manifest::build_mobile_scene_project_json;
pub(crate) use mobile_input::apply_mobile_scene_input_compat;
pub(crate) use package_input::read_packaged_scene_document;
pub use package_input::{
    PreparedPackagedScene, ScenePackageEntry, ScenePackageUnpackBackend, ScenePackageUnpackLimits,
    ScenePackageUnpackReport, ScenePackageUnpackRequest, package_entry_is_raw_scene_pkg,
    prepare_packaged_scene_source, unpack_scene_package_checked, validate_unpacked_scene_tree,
};
pub use references::{SceneReferenceReport, validate_scene_references};
pub use source_tree::{
    SceneSourceEntry, SceneSourceLimits, SceneSourceTree, inventory_scene_source,
};
