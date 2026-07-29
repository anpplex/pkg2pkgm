use std::path::{Path, PathBuf};

use pkg2mpkg_core::{Result, WallpaperKind, inspect_source};
use serde::Serialize;
use serde_json::Value;

use crate::output;

#[derive(Serialize)]
struct InspectOutput {
    root: PathBuf,
    project_file: Option<PathBuf>,
    entry_file: PathBuf,
    title: String,
    kind: WallpaperKind,
    manifest: Value,
}

pub fn run(input: &Path, json: bool) -> Result<()> {
    let source = inspect_source(input)?;
    let result = InspectOutput {
        root: source.root,
        project_file: source.project_file,
        entry_file: source.entry_file,
        title: source.title,
        kind: source.kind,
        manifest: source.manifest.into_raw(),
    };
    if json {
        output::print_json(&result)
    } else {
        output::print_text(&format!(
            "title: {}\ntype: {}\nentry: {}",
            result.title,
            result.kind.as_str(),
            result.entry_file.display()
        ))
    }
}
