//! Semantic verification of a staged Scene MPKG before atomic publish.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use crate::{ContainerVersion, Error, MpkgArchive, Result, mpkg::path::normalize_archive_path};

/// Magic prefix required of every converted TEX payload.
pub(crate) const TEX_V5_MAGIC: &[u8; 9] = b"TEXV0005\0";

/// Expected contents of a SceneDynamic package used by the staged verifier.
#[derive(Debug, Clone)]
pub(crate) struct ScenePackageExpectation {
    pub scene_entry: String,
    pub project_json: Vec<u8>,
    /// Archive paths sorted by UTF-8 bytes (exact package order).
    pub expected_paths: Vec<String>,
    /// Non-TEX payloads keyed by archive path (project.json excluded).
    pub non_tex_payloads: HashMap<String, Vec<u8>>,
    /// TEX archive paths that must open as TEXV0005.
    pub tex_paths: HashSet<String>,
    /// Local Scene references that must be present in the archive.
    pub local_references: Vec<String>,
}

/// Reopen a staged MPKG and validate SceneDynamic publish requirements.
pub(crate) fn verify_staged_scene_package(
    staged: &Path,
    expectation: &ScenePackageExpectation,
) -> Result<()> {
    let archive = MpkgArchive::open(staged)?;
    if archive.version() != ContainerVersion::Pkgm0020 {
        return Err(Error::VerificationFailed {
            reason: format!(
                "staged package version is {:?}, expected PKGM0020",
                archive.version()
            ),
        });
    }

    let paths: Vec<String> = archive
        .entries()
        .iter()
        .map(|entry| entry.path.clone())
        .collect();

    if paths != expectation.expected_paths {
        return Err(Error::VerificationFailed {
            reason: format!(
                "staged package path set/order mismatch: got {paths:?}, expected {:?}",
                expectation.expected_paths
            ),
        });
    }

    let mut sorted = paths.clone();
    sorted.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    if paths != sorted {
        return Err(Error::VerificationFailed {
            reason: "staged package entries are not bytewise ordered".into(),
        });
    }

    for path in &paths {
        if is_forbidden_archive_path(path) {
            return Err(Error::VerificationFailed {
                reason: format!("staged package contains forbidden path: {path}"),
            });
        }
    }

    let project_count = paths
        .iter()
        .filter(|path| path.as_str() == "project.json")
        .count();
    if project_count != 1 {
        return Err(Error::VerificationFailed {
            reason: format!(
                "staged package must contain exactly one project.json, found {project_count}"
            ),
        });
    }

    if !paths.iter().any(|path| path == &expectation.scene_entry) {
        return Err(Error::VerificationFailed {
            reason: format!(
                "staged package is missing declared Scene entry: {}",
                expectation.scene_entry
            ),
        });
    }

    let project_bytes = archive.read_entry("project.json")?;
    if project_bytes != expectation.project_json {
        return Err(Error::VerificationFailed {
            reason: "staged project.json bytes do not match the mobile construction".into(),
        });
    }

    let project_value: serde_json::Value =
        serde_json::from_slice(project_bytes.strip_suffix(b"\n").unwrap_or(&project_bytes))
            .map_err(|source| Error::VerificationFailed {
                reason: format!("staged project.json is not valid JSON: {source}"),
            })?;
    match project_value.get("type") {
        Some(serde_json::Value::String(kind)) if kind.eq_ignore_ascii_case("scene") => {}
        Some(serde_json::Value::String(kind)) => {
            return Err(Error::VerificationFailed {
                reason: format!("staged project.json type must be scene, got {kind}"),
            });
        }
        None | Some(serde_json::Value::Null) => {
            // Type may be omitted when entry implies Scene; still require Scene entry.
        }
        _ => {
            return Err(Error::VerificationFailed {
                reason: "staged project.json type must be a string when present".into(),
            });
        }
    }
    let entry = project_value
        .get("file")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::VerificationFailed {
            reason: "staged project.json is missing a string file field".into(),
        })?;
    let entry = normalize_archive_path(entry).map_err(|path| Error::VerificationFailed {
        reason: format!("staged project.json has unsafe file field: {path:?}"),
    })?;
    if entry != expectation.scene_entry {
        return Err(Error::VerificationFailed {
            reason: format!(
                "staged project.json file {entry} does not match scene entry {}",
                expectation.scene_entry
            ),
        });
    }

    for (path, expected) in &expectation.non_tex_payloads {
        let actual = archive.read_entry(path)?;
        if actual != *expected {
            return Err(Error::VerificationFailed {
                reason: format!("non-TEX payload mismatch for {path}"),
            });
        }
    }

    for path in &expectation.tex_paths {
        let bytes = archive.read_entry(path)?;
        if !bytes.starts_with(TEX_V5_MAGIC) {
            return Err(Error::VerificationFailed {
                reason: format!("converted TEX is not TEXV0005 for {path}"),
            });
        }
    }

    let path_set: HashSet<&str> = paths.iter().map(String::as_str).collect();
    for reference in &expectation.local_references {
        if !path_set.contains(reference.as_str()) {
            return Err(Error::VerificationFailed {
                reason: format!(
                    "Task 4 local reference is missing from staged archive: {reference}"
                ),
            });
        }
    }

    Ok(())
}

fn is_forbidden_archive_path(path: &str) -> bool {
    const SUFFIXES: &[&str] = &[".tex-json", ".mpkg", ".partial"];
    let bytes = path.as_bytes();
    for suffix in SUFFIXES {
        let suffix_bytes = suffix.as_bytes();
        if bytes.len() >= suffix_bytes.len() {
            let tail = &bytes[bytes.len() - suffix_bytes.len()..];
            if tail.eq_ignore_ascii_case(suffix_bytes) {
                return true;
            }
        }
    }
    path.split('/').any(|component| {
        component.len() >= ".pkg2mpkg-".len()
            && component.as_bytes()[..".pkg2mpkg-".len()].eq_ignore_ascii_case(b".pkg2mpkg-")
    })
}
