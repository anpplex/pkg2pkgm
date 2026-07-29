mod kind;
mod manifest;
mod source;

pub use kind::WallpaperKind;
pub use manifest::ProjectManifest;
pub use source::{SourceProject, inspect_source, source_requires_package_unpack};
