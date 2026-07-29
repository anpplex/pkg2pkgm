//! Execute an [`ExportPlan`] and publish a verified MPKG atomically.

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    ContainerVersion, Error, ExportMode, ExportPlan, HelperRequirement, MpkgBuilder,
    OverwritePolicy, ResourceTranscodeBackend, Result, ScenePackageUnpackBackend,
    ScenePackageUnpackLimits, SceneSourceLimits, SceneSourceTree, SourceProject, Stage,
    TextureTranscodeRequest, WallpaperKind, WriteReport, apply_compat_shaders,
    build_mobile_scene_project_json, inspect_source, inventory_scene_source,
    package_entry_is_raw_scene_pkg, prepare_packaged_scene_source, sanitize_mobile_properties,
    scene::apply_mobile_scene_input_compat,
    scene::package::{ScenePackageExpectation, verify_staged_scene_package},
    source_requires_package_unpack, transcode_texture_checked, validate_scene_references,
    workshop_project_id,
};

/// Default inventory bounds for SceneDynamic export (below the 4 GiB package cap).
const DEFAULT_SCENE_LIMITS: SceneSourceLimits = SceneSourceLimits {
    max_files: 100_000,
    max_file_bytes: 512 * 1024 * 1024,
    max_total_bytes: 3 * 1024 * 1024 * 1024,
};

/// Default unpack bounds for packaged Scene input (MPKG-aligned entry/path ceilings).
const DEFAULT_UNPACK_LIMITS: ScenePackageUnpackLimits = ScenePackageUnpackLimits {
    max_entries: 1_000_000,
    max_path_length: 16_384,
    max_file_bytes: 512 * 1024 * 1024,
    max_total_bytes: 3 * 1024 * 1024 * 1024,
};

/// Canonical path and open identity of the Scene root selected by reinspection.
/// The open handle prevents path retargeting from silently changing which tree
/// later output-safety checks protect.
struct BoundSceneRoot {
    canonical_path: PathBuf,
    identity: same_file::Handle,
}

/// Private export context for injecting optional helper backends and roots.
pub struct ExportContext<'a> {
    resource_backend: Option<&'a dyn ResourceTranscodeBackend>,
    package_unpack_backend: Option<&'a dyn ScenePackageUnpackBackend>,
    /// WE runtime `assets/zcompat/scene/shaders` root for optional shader overrides.
    compat_shader_root: Option<&'a Path>,
    /// Optional workshop project id when the manifest has none / for tests.
    project_id_override: Option<&'a str>,
}

impl<'a> ExportContext<'a> {
    /// Context with no backends or zcompat root declared.
    pub const fn new() -> Self {
        Self {
            resource_backend: None,
            package_unpack_backend: None,
            compat_shader_root: None,
            project_id_override: None,
        }
    }

    /// Context that injects the ResourceTranscode backend used for `.tex` work.
    pub fn with_resource_backend(backend: &'a dyn ResourceTranscodeBackend) -> Self {
        Self {
            resource_backend: Some(backend),
            package_unpack_backend: None,
            compat_shader_root: None,
            project_id_override: None,
        }
    }

    /// Context that injects only the Scene package unpack backend.
    pub fn with_package_unpack_backend(backend: &'a dyn ScenePackageUnpackBackend) -> Self {
        Self {
            resource_backend: None,
            package_unpack_backend: Some(backend),
            compat_shader_root: None,
            project_id_override: None,
        }
    }

    /// Fluent setter for the ResourceTranscode backend.
    pub fn resource_backend(mut self, backend: &'a dyn ResourceTranscodeBackend) -> Self {
        self.resource_backend = Some(backend);
        self
    }

    /// Fluent setter for the Scene package unpack backend.
    pub fn package_unpack_backend(mut self, backend: &'a dyn ScenePackageUnpackBackend) -> Self {
        self.package_unpack_backend = Some(backend);
        self
    }

    /// Fluent setter for the WE runtime zcompat shaders root
    /// (`assets/zcompat/scene/shaders`). Applied only to the task-owned
    /// snapshot tree; the user's original source is never mutated.
    pub fn compat_shader_root(mut self, path: &'a Path) -> Self {
        self.compat_shader_root = Some(path);
        self
    }

    /// Fluent setter for an explicit workshop project id used when resolving
    /// zcompat rules (overrides `workshopid` from the project manifest).
    pub fn project_id_override(mut self, project_id: &'a str) -> Self {
        self.project_id_override = Some(project_id);
        self
    }
}

impl Default for ExportContext<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// Deterministic, serializable summary of a successful export.
///
/// Contains no timestamps, elapsed time, task temp paths, process IDs, or
/// helper diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportReport {
    pub source: PathBuf,
    pub output: PathBuf,
    pub kind: WallpaperKind,
    pub mode: ExportMode,
    pub container_version: ContainerVersion,
    pub entry_count: usize,
    pub output_bytes: u64,
    pub texture_count: usize,
    pub texture_input_bytes: u64,
    pub texture_output_bytes: u64,
}

/// Execute `plan` against `source` and atomically publish a verified MPKG.
pub fn execute_export_plan(
    source: &SourceProject,
    plan: &ExportPlan,
    context: &ExportContext<'_>,
    overwrite: OverwritePolicy,
) -> Result<ExportReport> {
    match source.kind {
        WallpaperKind::Web | WallpaperKind::Application => {
            return Err(Error::unsupported_type(source.kind.as_str()));
        }
        WallpaperKind::Scene | WallpaperKind::Video => {}
    }

    match plan.mode {
        ExportMode::SceneDynamic {
            compression,
            reduction,
        } => execute_scene_dynamic(source, plan, context, overwrite, compression, reduction),
        ExportMode::ScenePreRenderedVideo => Err(Error::BackendUnavailable {
            backend: "scene_capture+h264_encode".into(),
        }),
        ExportMode::Video { .. } => Err(Error::BackendUnavailable {
            backend: "video_export".into(),
        }),
    }
}

fn execute_scene_dynamic(
    source: &SourceProject,
    plan: &ExportPlan,
    context: &ExportContext<'_>,
    overwrite: OverwritePolicy,
    compression: crate::Compression,
    reduction: crate::Reduction,
) -> Result<ExportReport> {
    if source.kind != WallpaperKind::Scene || plan.kind != WallpaperKind::Scene {
        return Err(Error::InvalidArguments {
            reason: format!(
                "SceneDynamic export requires Scene source and plan, got source={} plan={}",
                source.kind.as_str(),
                plan.kind.as_str()
            ),
        });
    }

    let live = reinspect_and_match(source, plan)?;
    let bound_root = bind_scene_root(&live, &plan.output)?;
    reject_output_overlaps_scene_source(&live, &bound_root, &plan.output)?;

    // Require the declared ResourceTranscode backend before snapshotting or
    // conversion begins, even when the project has zero TEX entries.
    let backend = context
        .resource_backend
        .ok_or_else(|| Error::BackendUnavailable {
            backend: helper_name(HelperRequirement::ResourceTranscode).into(),
        })?;
    if !plan.helpers.contains(&HelperRequirement::ResourceTranscode) {
        return Err(Error::BackendUnavailable {
            backend: helper_name(HelperRequirement::ResourceTranscode).into(),
        });
    }

    if overwrite == OverwritePolicy::Deny && plan.output.exists() {
        return Err(Error::Io {
            stage: Stage::Pack,
            path: plan.output.clone(),
            source: std::io::Error::new(std::io::ErrorKind::AlreadyExists, "output already exists"),
        });
    }

    // Task-private directory: unpacked/ (optional) + snapshot/ + converted/.
    let task_dir = tempfile::Builder::new()
        .prefix(".pkg2mpkg-export-")
        .tempdir()
        .map_err(|source_err| Error::Io {
            stage: Stage::Pack,
            path: PathBuf::from(".pkg2mpkg-export"),
            source: source_err,
        })?;

    let (working_source, inventory) = prepare_scene_working_tree(&live, context, task_dir.path())?;

    let snapshot_root = task_dir.path().join("snapshot");
    let converted_root = task_dir.path().join("converted");
    fs::create_dir_all(&snapshot_root).map_err(|source_err| Error::Io {
        stage: Stage::Pack,
        path: snapshot_root.clone(),
        source: source_err,
    })?;
    fs::create_dir_all(&converted_root).map_err(|source_err| Error::Io {
        stage: Stage::Pack,
        path: converted_root.clone(),
        source: source_err,
    })?;

    // Copy every inventory entry into the snapshot; never re-read original paths.
    // Raw `.pkg` must never enter the snapshot / final MPKG.
    let mut snapshot_entries: Vec<(String, PathBuf, u64)> =
        Vec::with_capacity(inventory.entries.len());
    for entry in &inventory.entries {
        if package_entry_is_raw_scene_pkg(&entry.archive_path) {
            return Err(Error::ConversionFailed {
                reason: format!(
                    "raw package path must not enter Android MPKG: {}",
                    entry.archive_path
                ),
            });
        }
        let dest = snapshot_root.join(&entry.archive_path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|source_err| Error::Io {
                stage: Stage::Pack,
                path: parent.to_path_buf(),
                source: source_err,
            })?;
        }
        fs::copy(&entry.source_path, &dest).map_err(|source_err| Error::Io {
            stage: Stage::Pack,
            path: dest.clone(),
            source: source_err,
        })?;
        let copied = fs::metadata(&dest).map_err(|source_err| Error::Io {
            stage: Stage::Pack,
            path: dest.clone(),
            source: source_err,
        })?;
        if copied.len() != entry.size {
            return Err(Error::ConversionFailed {
                reason: format!(
                    "snapshot size mismatch for {}: expected {}, got {}",
                    entry.archive_path,
                    entry.size,
                    copied.len()
                ),
            });
        }
        snapshot_entries.push((entry.archive_path.clone(), dest, entry.size));
    }

    let snapshot_source = SourceProject {
        root: snapshot_root.clone(),
        project_file: Some(snapshot_root.join("project.json")),
        entry_file: snapshot_root.join(&inventory.scene_entry),
        title: working_source.title.clone(),
        kind: WallpaperKind::Scene,
        manifest: working_source.manifest.clone(),
    };

    // Optional zcompat shader overrides: mutate only the task-owned snapshot.
    // Missing root, missing project id, or non-matching rules are no-ops.
    if let Some(zcompat_root) = context.compat_shader_root {
        let project_id = context
            .project_id_override
            .map(str::to_owned)
            .or_else(|| workshop_project_id(&working_source));
        if let Some(project_id) = project_id {
            apply_compat_shaders(&snapshot_root, &project_id, zcompat_root)?;
        }
    }

    // Android live wallpapers deliver cursor/touch primarily via postprocess;
    // rewrite desktop cursorDown bodies onto shared.doJump + postprocess relay.
    apply_mobile_scene_input_compat(&snapshot_root, &inventory.scene_entry)?;

    let project_json = build_mobile_scene_project_json(&snapshot_source, plan)?;
    let references = validate_scene_references(&snapshot_root, &inventory.scene_entry)?;

    // Convert each ASCII-case-insensitive .tex exactly once.
    let mut tex_converted: HashMap<String, PathBuf> = HashMap::new();
    let mut texture_count = 0usize;
    let mut texture_input_bytes = 0u64;
    let mut texture_output_bytes = 0u64;
    let mut ordinal = 0u32;

    for (archive_path, snapshot_path, _) in &snapshot_entries {
        if !is_tex_archive_path(archive_path) {
            continue;
        }
        let output_name = format!("{ordinal:06}.tex");
        ordinal = ordinal.saturating_add(1);
        let converted_path = converted_root.join(&output_name);
        let request = TextureTranscodeRequest::new(
            snapshot_path.clone(),
            converted_path.clone(),
            compression,
            reduction,
        )?;
        let report = transcode_texture_checked(backend, &request)?;
        // Ensure converted payload exists and is non-empty enough for later verify.
        let meta = fs::metadata(&converted_path).map_err(|source_err| Error::ConversionFailed {
            reason: format!(
                "converted TEX missing after backend success: {} ({source_err})",
                converted_path.display()
            ),
        })?;
        if meta.len() != report.output_bytes {
            return Err(Error::ConversionFailed {
                reason: format!(
                    "converted TEX size {} does not match report {}",
                    meta.len(),
                    report.output_bytes
                ),
            });
        }
        texture_count += 1;
        texture_input_bytes = texture_input_bytes.saturating_add(report.input_bytes);
        texture_output_bytes = texture_output_bytes.saturating_add(report.output_bytes);
        tex_converted.insert(archive_path.clone(), converted_path);
    }

    // Build pending package entries: mobile project.json + snapshot non-TEX + converted TEX.
    enum PendingSource {
        Bytes(Vec<u8>),
        File(PathBuf),
    }
    struct Pending {
        archive_path: String,
        source: PendingSource,
    }

    let mut pending: Vec<Pending> = Vec::new();
    let mut non_tex_payloads: HashMap<String, Vec<u8>> = HashMap::new();
    let mut tex_paths: HashSet<String> = HashSet::new();

    pending.push(Pending {
        archive_path: "project.json".into(),
        source: PendingSource::Bytes(project_json.clone()),
    });

    for (archive_path, snapshot_path, _) in &snapshot_entries {
        if archive_path == "project.json" {
            continue;
        }
        if let Some(converted) = tex_converted.get(archive_path) {
            tex_paths.insert(archive_path.clone());
            pending.push(Pending {
                archive_path: archive_path.clone(),
                source: PendingSource::File(converted.clone()),
            });
        } else {
            let bytes = fs::read(snapshot_path).map_err(|source_err| Error::Io {
                stage: Stage::Pack,
                path: snapshot_path.clone(),
                source: source_err,
            })?;
            non_tex_payloads.insert(archive_path.clone(), bytes.clone());
            pending.push(Pending {
                archive_path: archive_path.clone(),
                source: PendingSource::File(snapshot_path.clone()),
            });
        }
    }

    pending.sort_by(|left, right| {
        left.archive_path
            .as_bytes()
            .cmp(right.archive_path.as_bytes())
    });
    let expected_paths: Vec<String> = pending
        .iter()
        .map(|entry| entry.archive_path.clone())
        .collect();

    let mut builder = MpkgBuilder::new(ContainerVersion::Pkgm0020);
    for entry in &pending {
        match &entry.source {
            PendingSource::Bytes(bytes) => {
                builder.add_bytes(&entry.archive_path, bytes.clone())?;
            }
            PendingSource::File(path) => {
                // Never pass original source paths — only snapshot/converted.
                builder.add_file(&entry.archive_path, path)?;
            }
        }
    }

    let expectation = ScenePackageExpectation {
        scene_entry: inventory.scene_entry.clone(),
        project_json,
        expected_paths,
        non_tex_payloads,
        tex_paths,
        local_references: references.local_references,
    };

    let write_report: WriteReport = builder.write_atomic_verified_before_publish(
        &plan.output,
        overwrite,
        |staged| verify_staged_scene_package(staged, &expectation),
        || {
            // This closure runs after the writer's generic staged verification
            // and immediately before persist. Keep the originally bound source
            // root authoritative even if helpers retarget path symlinks.
            reject_output_overlaps_scene_source(&live, &bound_root, &plan.output)
        },
    )?;

    // task_dir drops here and removes snapshot/converted.
    drop(task_dir);

    Ok(ExportReport {
        source: plan.source.clone(),
        output: write_report.output,
        kind: WallpaperKind::Scene,
        mode: plan.mode,
        container_version: ContainerVersion::Pkgm0020,
        entry_count: write_report.entries,
        output_bytes: write_report.bytes,
        texture_count,
        texture_input_bytes,
        texture_output_bytes,
    })
}

/// Reject a Scene output that could replace any part of its input tree.
///
/// The canonical location check covers direct descendants and destinations
/// reached through symlinked parent directories. The identity scan covers an
/// existing destination outside the tree that is a hard-link (or other file
/// identity alias) to a source file.
fn reject_output_overlaps_scene_source(
    source: &SourceProject,
    bound_root: &BoundSceneRoot,
    output: &Path,
) -> Result<()> {
    verify_scene_root_binding(source, bound_root, output)?;
    let output_location = resolve_output_entry_location(output)?;
    if output_location.starts_with(&bound_root.canonical_path) {
        return Err(Error::InvalidArguments {
            reason: format!(
                "output must be outside the Scene source root: output={} source={}",
                output.display(),
                source.root.display()
            ),
        });
    }

    match fs::symlink_metadata(output) {
        Ok(_) => {
            if output_aliases_regular_file_in_tree(&bound_root.canonical_path, output)? {
                return Err(Error::InvalidArguments {
                    reason: format!(
                        "output aliases a file in the Scene source tree: output={} source={}",
                        output.display(),
                        source.root.display()
                    ),
                });
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(Error::InvalidArguments {
                reason: format!("cannot inspect output path {}: {error}", output.display()),
            });
        }
    }

    Ok(())
}

fn bind_scene_root(source: &SourceProject, output: &Path) -> Result<BoundSceneRoot> {
    let canonical_path = fs::canonicalize(&source.root).map_err(|error| {
        output_safety_error(
            format!("cannot canonicalize Scene source root: {error}"),
            &source.root,
            output,
        )
    })?;
    let identity = same_file::Handle::from_path(&canonical_path).map_err(|error| {
        output_safety_error(
            format!("cannot bind Scene source root identity: {error}"),
            &source.root,
            output,
        )
    })?;
    let current = same_file::Handle::from_path(&source.root).map_err(|error| {
        output_safety_error(
            format!("cannot verify Scene source root identity: {error}"),
            &source.root,
            output,
        )
    })?;
    if current != identity {
        return Err(output_safety_error(
            "Scene source root changed while binding output safety",
            &source.root,
            output,
        ));
    }
    Ok(BoundSceneRoot {
        canonical_path,
        identity,
    })
}

fn verify_scene_root_binding(
    source: &SourceProject,
    bound_root: &BoundSceneRoot,
    output: &Path,
) -> Result<()> {
    let current_identity = same_file::Handle::from_path(&source.root).map_err(|error| {
        output_safety_error(
            format!("cannot reopen Scene source root identity: {error}"),
            &source.root,
            output,
        )
    })?;
    let current_path = fs::canonicalize(&source.root).map_err(|error| {
        output_safety_error(
            format!("cannot recanonicalize Scene source root: {error}"),
            &source.root,
            output,
        )
    })?;
    let confirmed_identity = same_file::Handle::from_path(&source.root).map_err(|error| {
        output_safety_error(
            format!("cannot confirm Scene source root identity: {error}"),
            &source.root,
            output,
        )
    })?;
    if current_identity != bound_root.identity
        || confirmed_identity != bound_root.identity
        || current_path != bound_root.canonical_path
    {
        return Err(output_safety_error(
            format!(
                "Scene source root changed after inspection; bound={}",
                bound_root.canonical_path.display()
            ),
            &source.root,
            output,
        ));
    }
    Ok(())
}

fn output_safety_error(reason: impl AsRef<str>, source_root: &Path, output: &Path) -> Error {
    Error::InvalidArguments {
        reason: format!(
            "{}: output={} source={}",
            reason.as_ref(),
            output.display(),
            source_root.display()
        ),
    }
}

/// Resolve the directory entry that atomic publication will replace. The
/// final component is deliberately not canonicalized: an output symlink inside
/// the source tree is still a source-tree directory entry even if its target is
/// outside the tree.
fn resolve_output_entry_location(output: &Path) -> Result<PathBuf> {
    let file_name = output.file_name().ok_or_else(|| Error::InvalidArguments {
        reason: format!("output must name a resolvable file: {}", output.display()),
    })?;
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut location = resolve_path_location(parent, output)?;
    location.push(file_name);
    Ok(location)
}

/// Resolve a path through its deepest existing ancestor, retaining any
/// trailing non-existent components.
fn resolve_path_location(path: &Path, output: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| Error::InvalidArguments {
                reason: format!(
                    "cannot resolve relative output {}: {error}",
                    output.display()
                ),
            })?
            .join(path)
    };

    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => {
                let mut resolved =
                    fs::canonicalize(ancestor).map_err(|error| Error::InvalidArguments {
                        reason: format!("cannot resolve output path {}: {error}", output.display()),
                    })?;
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let name = ancestor
                    .file_name()
                    .ok_or_else(|| Error::InvalidArguments {
                        reason: format!("output has no resolvable parent: {}", output.display()),
                    })?;
                missing.push(name.to_os_string());
                ancestor = ancestor.parent().ok_or_else(|| Error::InvalidArguments {
                    reason: format!("output has no resolvable parent: {}", output.display()),
                })?;
            }
            Err(error) => {
                return Err(Error::InvalidArguments {
                    reason: format!("cannot inspect output path {}: {error}", output.display()),
                });
            }
        }
    }
}

fn output_aliases_regular_file_in_tree(root: &Path, output: &Path) -> Result<bool> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory).map_err(|error| Error::InvalidArguments {
            reason: format!(
                "cannot validate source directory {} against output {}: {error}",
                directory.display(),
                output.display()
            ),
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| Error::InvalidArguments {
                reason: format!(
                    "cannot validate source directory {} against output {}: {error}",
                    directory.display(),
                    output.display()
                ),
            })?;
            let path = entry.path();
            let metadata =
                fs::symlink_metadata(&path).map_err(|error| Error::InvalidArguments {
                    reason: format!(
                        "cannot inspect source path {} against output {}: {error}",
                        path.display(),
                        output.display()
                    ),
                })?;
            if metadata.file_type().is_dir() {
                pending.push(path);
            } else if metadata.file_type().is_file()
                && same_file::is_same_file(&path, output).map_err(|error| {
                    Error::InvalidArguments {
                        reason: format!(
                            "cannot compare source path {} with output {}: {error}",
                            path.display(),
                            output.display()
                        ),
                    }
                })?
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Resolve the inventory tree used for snapshotting: either unpack a packaged
/// Scene into `task_dir/unpacked`, or inventory a loose Scene tree in place.
fn prepare_scene_working_tree(
    live: &SourceProject,
    context: &ExportContext<'_>,
    task_dir: &Path,
) -> Result<(SourceProject, SceneSourceTree)> {
    if source_requires_package_unpack(live) {
        let unpack_backend =
            context
                .package_unpack_backend
                .ok_or_else(|| Error::BackendUnavailable {
                    backend: "scene_pkg_unpack".into(),
                })?;
        let unpack_dir = task_dir.join("unpacked");
        fs::create_dir_all(&unpack_dir).map_err(|source_err| Error::Io {
            stage: Stage::Unpack,
            path: unpack_dir.clone(),
            source: source_err,
        })?;
        let prepared = prepare_packaged_scene_source(
            live,
            unpack_backend,
            &unpack_dir,
            DEFAULT_UNPACK_LIMITS,
            DEFAULT_SCENE_LIMITS,
        )?;
        if package_entry_is_raw_scene_pkg(&prepared.tree.scene_entry)
            || prepared
                .tree
                .entries
                .iter()
                .any(|entry| package_entry_is_raw_scene_pkg(&entry.archive_path))
        {
            return Err(Error::ConversionFailed {
                reason: "prepared packaged scene still contains a raw .pkg path".into(),
            });
        }
        return Ok((prepared.source, prepared.tree));
    }

    // Loose Scene path: still reject a raw package entry if present without
    // going through the unpack backend (defensive double-check).
    reject_raw_scene_pkg(live)?;
    let inventory = inventory_scene_source(live, DEFAULT_SCENE_LIMITS)?;
    reject_raw_scene_pkg_entry(&inventory.scene_entry)?;
    if inventory
        .entries
        .iter()
        .any(|entry| package_entry_is_raw_scene_pkg(&entry.archive_path))
    {
        return Err(Error::BackendUnavailable {
            backend: "scene_pkg_unpack".into(),
        });
    }
    Ok((live.clone(), inventory))
}

fn reinspect_and_match(source: &SourceProject, plan: &ExportPlan) -> Result<SourceProject> {
    let inspect_path = source
        .project_file
        .as_deref()
        .map(|path| {
            path.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| source.root.clone())
        })
        .unwrap_or_else(|| source.root.clone());

    let live = inspect_source(&inspect_path)?;

    if live.kind != WallpaperKind::Scene {
        return Err(Error::InvalidArguments {
            reason: format!(
                "re-inspected source is {}, expected scene",
                live.kind.as_str()
            ),
        });
    }

    let live_root = canonicalize_path(&live.root)?;
    let source_root = canonicalize_path(&source.root)?;
    let plan_root = canonicalize_path(&plan.source).map_err(|error| {
        // Plan root that cannot be canonicalized is a mismatch / bad argument.
        match error {
            Error::Io { .. } => Error::InvalidArguments {
                reason: format!(
                    "plan source root cannot be canonicalized: {}",
                    plan.source.display()
                ),
            },
            other => other,
        }
    })?;

    if live_root != source_root || live_root != plan_root {
        return Err(Error::InvalidArguments {
            reason: format!(
                "source/plan root mismatch: live={}, source={}, plan={}",
                live_root.display(),
                source_root.display(),
                plan_root.display()
            ),
        });
    }

    let live_entry =
        live.entry_file
            .strip_prefix(&live.root)
            .map_err(|_| Error::InvalidArguments {
                reason: format!(
                    "re-inspected physical source entry is outside its root: {}",
                    live.entry_file.display()
                ),
            })?;
    let source_entry =
        source
            .entry_file
            .strip_prefix(&source.root)
            .map_err(|_| Error::InvalidArguments {
                reason: format!(
                    "inspected physical source entry is outside its root: {}",
                    source.entry_file.display()
                ),
            })?;
    if live_entry != source_entry {
        return Err(Error::InvalidArguments {
            reason: format!(
                "physical source entry changed since inspect: was {}, now {}",
                source_entry.display(),
                live_entry.display()
            ),
        });
    }

    let canonical_live_entry = canonicalize_path(&live.entry_file)?;
    if !canonical_live_entry.starts_with(&live_root) {
        return Err(Error::InvalidArguments {
            reason: format!(
                "re-inspected physical source entry resolves outside its root: {}",
                canonical_live_entry.display()
            ),
        });
    }

    if live.kind != plan.kind || source.kind != plan.kind {
        return Err(Error::InvalidArguments {
            reason: format!(
                "source/plan kind mismatch: live={}, plan={}",
                live.kind.as_str(),
                plan.kind.as_str()
            ),
        });
    }

    if live.title != plan.title || source.title != plan.title {
        return Err(Error::InvalidArguments {
            reason: format!(
                "source/plan title mismatch: live={:?}, plan={:?}",
                live.title, plan.title
            ),
        });
    }

    let live_entry = live.manifest.entry().unwrap_or("");
    let source_entry = source.manifest.entry().unwrap_or("");
    if live_entry != source_entry {
        return Err(Error::InvalidArguments {
            reason: format!(
                "source entry changed since inspect: was {source_entry:?}, now {live_entry:?}"
            ),
        });
    }

    let expected_properties = sanitized_properties_from(&live);
    if expected_properties != plan.properties {
        return Err(Error::InvalidArguments {
            reason: "source/plan sanitized properties mismatch".into(),
        });
    }

    Ok(live)
}

fn sanitized_properties_from(
    source: &SourceProject,
) -> indexmap::IndexMap<String, serde_json::Value> {
    let properties = source
        .manifest
        .raw()
        .pointer("/general/properties")
        .and_then(serde_json::Value::as_object)
        .or_else(|| {
            source
                .manifest
                .raw()
                .get("properties")
                .and_then(serde_json::Value::as_object)
        });
    let Some(properties) = properties else {
        return indexmap::IndexMap::new();
    };
    let mut entries: Vec<_> = sanitize_mobile_properties(properties).into_iter().collect();
    entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    entries.into_iter().collect()
}

fn reject_raw_scene_pkg(source: &SourceProject) -> Result<()> {
    if let Some(entry) = source.manifest.entry() {
        reject_raw_scene_pkg_entry(entry)?;
    }
    Ok(())
}

fn reject_raw_scene_pkg_entry(entry: &str) -> Result<()> {
    if package_entry_is_raw_scene_pkg(entry) {
        return Err(Error::BackendUnavailable {
            backend: "scene_pkg_unpack".into(),
        });
    }
    Ok(())
}

fn is_tex_archive_path(path: &str) -> bool {
    has_ascii_extension(path, "tex")
}

fn has_ascii_extension(path: &str, expected: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

fn canonicalize_path(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).map_err(|source| Error::Io {
        stage: Stage::Inspect,
        path: path.to_path_buf(),
        source,
    })
}

fn helper_name(requirement: HelperRequirement) -> &'static str {
    match requirement {
        HelperRequirement::ResourceTranscode => "resource_transcode",
        HelperRequirement::SceneCapture => "scene_capture",
        HelperRequirement::H264Encode => "h264_encode",
    }
}
