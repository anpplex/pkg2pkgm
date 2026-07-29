use std::path::Path;

use pkg2mpkg_core::{Error, MpkgArchive, ProjectManifest, Result, WallpaperKind};
use serde::Serialize;

use crate::output;

#[derive(Serialize)]
struct VerifyOutput {
    version: String,
    entry_count: usize,
    project_type: WallpaperKind,
    entries: Vec<String>,
}

pub fn run(input: &Path, json: bool) -> Result<()> {
    let archive = MpkgArchive::open(input)?;
    let project_bytes = archive.read_entry("project.json")?;
    let manifest = ProjectManifest::parse(&project_bytes)
        .map_err(|error| Error::invalid_mpkg(format!("project.json: {error}")))?;
    let kind = manifest
        .kind()
        .map_err(|error| Error::invalid_mpkg(format!("project.json: {error}")))?;
    match kind {
        WallpaperKind::Web | WallpaperKind::Application => {
            return Err(Error::unsupported_type(kind.as_str()));
        }
        WallpaperKind::Scene | WallpaperKind::Video => {}
    }
    let result = VerifyOutput {
        version: archive.version().as_magic().into_owned(),
        entry_count: archive.entries().len(),
        project_type: kind,
        entries: archive
            .entries()
            .iter()
            .map(|entry| entry.path.clone())
            .collect(),
    };
    if json {
        output::print_json(&result)
    } else {
        output::print_text(&format!(
            "version: {}\nentries: {}\ntype: {}",
            result.version,
            result.entry_count,
            result.project_type.as_str()
        ))
    }
}
