//! Failure, race, and rejection paths for dynamic Scene export (Task 5).

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use pkg2mpkg_core::{
    ContentClass, ErrorCode, ExportContext, ExportMode, ExportRequest, HelperRequirement,
    OverwritePolicy, ProjectManifest, ResourceTranscodeBackend, SceneProfile, SourceProject, Stage,
    TextureTranscodeReport, TextureTranscodeRequest, VideoInputCompatibility, WallpaperKind,
    build_export_plan, execute_export_plan, inspect_source,
};
use pkg2mpkg_fixtures::{
    dynamic_scene_project, synthetic_application_project, synthetic_video_project,
    synthetic_web_project, write_bytes,
};
use tempfile::tempdir;

const TEX_V5_MAGIC: &[u8; 9] = b"TEXV0005\0";

struct FakeOkBackend;

impl ResourceTranscodeBackend for FakeOkBackend {
    fn transcode_texture(
        &self,
        request: &TextureTranscodeRequest,
    ) -> pkg2mpkg_core::Result<TextureTranscodeReport> {
        let input_bytes = fs::metadata(&request.input)
            .map(|meta| meta.len())
            .unwrap_or(0);
        let mut body = TEX_V5_MAGIC.to_vec();
        body.extend_from_slice(b"ok");
        if let Some(parent) = request.output.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&request.output, &body).map_err(|source| pkg2mpkg_core::Error::Io {
            stage: Stage::Convert,
            path: request.output.clone(),
            source,
        })?;
        Ok(TextureTranscodeReport {
            output: request.output.clone(),
            input_bytes,
            output_bytes: body.len() as u64,
            compression: request.compression,
            reduction: request.reduction,
        })
    }
}

struct CountingOkBackend {
    calls: Mutex<u32>,
}

impl CountingOkBackend {
    fn new() -> Self {
        Self {
            calls: Mutex::new(0),
        }
    }

    fn calls(&self) -> u32 {
        *self.calls.lock().unwrap()
    }
}

impl ResourceTranscodeBackend for CountingOkBackend {
    fn transcode_texture(
        &self,
        request: &TextureTranscodeRequest,
    ) -> pkg2mpkg_core::Result<TextureTranscodeReport> {
        *self.calls.lock().unwrap() += 1;
        FakeOkBackend.transcode_texture(request)
    }
}

struct FailingBackend;

impl ResourceTranscodeBackend for FailingBackend {
    fn transcode_texture(
        &self,
        _request: &TextureTranscodeRequest,
    ) -> pkg2mpkg_core::Result<TextureTranscodeReport> {
        Err(pkg2mpkg_core::Error::ConversionFailed {
            reason: "fake helper refused conversion".into(),
        })
    }
}

/// Nominally successful backend that writes invalid TEX magic for semantic fail.
struct InvalidTexBackend;

impl ResourceTranscodeBackend for InvalidTexBackend {
    fn transcode_texture(
        &self,
        request: &TextureTranscodeRequest,
    ) -> pkg2mpkg_core::Result<TextureTranscodeReport> {
        if let Some(parent) = request.output.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&request.output, b"NOT-A-VALID-TEX-HEADER").unwrap();
        Ok(TextureTranscodeReport {
            output: request.output.clone(),
            input_bytes: 8,
            output_bytes: 22,
            compression: request.compression,
            reduction: request.reduction,
        })
    }
}

fn dynamic_plan(source: &SourceProject, output: PathBuf) -> pkg2mpkg_core::ExportPlan {
    build_export_plan(
        source,
        ExportRequest::scene(output, SceneProfile::High, ContentClass::Normal),
    )
    .unwrap()
}

fn partials_in(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(read) = fs::read_dir(dir) {
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains("partial") || name.starts_with(".pkg2mpkg-") {
                names.push(name);
            }
        }
    }
    names
}

fn assert_replace_target_is_rejected_before_backend(relative_output: &str) {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let output = fixture.path().join(relative_output);
    let original = fs::read(&output).ok();
    let plan = dynamic_plan(&source, output.clone());
    let backend = CountingOkBackend::new();
    let context = ExportContext::with_resource_backend(&backend);

    let error =
        execute_export_plan(&source, &plan, &context, OverwritePolicy::Replace).unwrap_err();

    assert_eq!(error.code(), ErrorCode::InvalidArguments);
    assert!(
        error.to_string().contains("output") && error.to_string().contains("source"),
        "unexpected output-safety error: {error}"
    );
    assert_eq!(backend.calls(), 0, "unsafe output must fail before backend");
    match original {
        Some(bytes) => assert_eq!(fs::read(&output).unwrap(), bytes),
        None => assert!(!output.exists()),
    }
}

#[test]
fn replace_rejects_new_output_inside_scene_root_before_backend() {
    assert_replace_target_is_rejected_before_backend("mobile.mpkg");
}

#[test]
fn replace_rejects_project_manifest_output_before_backend() {
    assert_replace_target_is_rejected_before_backend("project.json");
}

#[test]
fn replace_rejects_scene_entry_output_before_backend() {
    assert_replace_target_is_rejected_before_backend("scene.json");
}

#[test]
fn replace_rejects_texture_output_before_backend() {
    assert_replace_target_is_rejected_before_backend("materials/opaque.tex");
}

#[cfg(unix)]
#[test]
fn replace_rejects_symlink_alias_to_source_file_before_backend() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("source-alias.mpkg");
    std::os::unix::fs::symlink(fixture.entry_path(), &output).unwrap();
    let original = fs::read(fixture.entry_path()).unwrap();
    let plan = dynamic_plan(&source, output.clone());
    let backend = CountingOkBackend::new();
    let context = ExportContext::with_resource_backend(&backend);

    let error =
        execute_export_plan(&source, &plan, &context, OverwritePolicy::Replace).unwrap_err();

    assert_eq!(error.code(), ErrorCode::InvalidArguments);
    assert_eq!(backend.calls(), 0);
    assert_eq!(fs::read(fixture.entry_path()).unwrap(), original);
    assert!(
        fs::symlink_metadata(&output)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[cfg(unix)]
#[test]
fn replace_rejects_output_symlink_located_inside_source_root_before_backend() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let outside = tempdir().unwrap();
    let outside_target = outside.path().join("unrelated-placeholder");
    fs::write(&outside_target, b"outside").unwrap();
    let output = fixture.path().join("inside-source.mpkg");
    std::os::unix::fs::symlink(&outside_target, &output).unwrap();
    let plan = dynamic_plan(&source, output.clone());
    let backend = CountingOkBackend::new();
    let context = ExportContext::with_resource_backend(&backend);

    let error =
        execute_export_plan(&source, &plan, &context, OverwritePolicy::Replace).unwrap_err();

    assert_eq!(error.code(), ErrorCode::InvalidArguments);
    assert_eq!(backend.calls(), 0);
    assert!(
        fs::symlink_metadata(&output)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(outside_target).unwrap(), b"outside");
}

#[cfg(unix)]
#[test]
fn replace_rejects_dangling_output_symlink_inside_source_root_before_backend() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let output = fixture.path().join("dangling-inside-source.mpkg");
    std::os::unix::fs::symlink("missing-target", &output).unwrap();
    let plan = dynamic_plan(&source, output.clone());
    let backend = CountingOkBackend::new();
    let context = ExportContext::with_resource_backend(&backend);

    let error =
        execute_export_plan(&source, &plan, &context, OverwritePolicy::Replace).unwrap_err();

    assert_eq!(error.code(), ErrorCode::InvalidArguments);
    assert_eq!(backend.calls(), 0);
    assert!(
        fs::symlink_metadata(&output)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[cfg(unix)]
#[test]
fn replace_rejects_output_reached_through_symlinked_source_parent() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let out_dir = tempdir().unwrap();
    let source_alias = out_dir.path().join("source-directory-alias");
    std::os::unix::fs::symlink(fixture.path(), &source_alias).unwrap();
    let output = source_alias.join("new-mobile.mpkg");
    let plan = dynamic_plan(&source, output.clone());
    let backend = CountingOkBackend::new();
    let context = ExportContext::with_resource_backend(&backend);

    let error =
        execute_export_plan(&source, &plan, &context, OverwritePolicy::Replace).unwrap_err();

    assert_eq!(error.code(), ErrorCode::InvalidArguments);
    assert_eq!(backend.calls(), 0);
    assert!(!fixture.path().join("new-mobile.mpkg").exists());
}

#[cfg(unix)]
#[test]
fn backend_cannot_retarget_source_and_output_symlinks_before_publish() {
    let workspace = tempdir().unwrap();
    let original_root = workspace.path().join("original-source");
    let decoy_root = workspace.path().join("decoy-source");
    fs::create_dir_all(&original_root).unwrap();
    fs::create_dir_all(&decoy_root).unwrap();
    let original_manifest = br#"{"title":"Retarget","type":"scene","file":"scene.json"}"#;
    write_bytes(&original_root.join("project.json"), original_manifest);
    write_bytes(&original_root.join("scene.json"), br#"{"objects":[]}"#);
    write_bytes(
        &original_root.join("materials/opaque.tex"),
        b"TEXV0005\0original",
    );

    let source_alias = workspace.path().join("source-alias");
    let output_parent_alias = workspace.path().join("output-parent-alias");
    std::os::unix::fs::symlink(&original_root, &source_alias).unwrap();
    std::os::unix::fs::symlink(&decoy_root, &output_parent_alias).unwrap();

    let source = inspect_source(&source_alias).unwrap();
    let output = output_parent_alias.join("project.json");
    let plan = dynamic_plan(&source, output);

    struct RetargetingBackend {
        source_alias: PathBuf,
        output_parent_alias: PathBuf,
        original_root: PathBuf,
        decoy_root: PathBuf,
        calls: Mutex<u32>,
    }
    impl ResourceTranscodeBackend for RetargetingBackend {
        fn transcode_texture(
            &self,
            request: &TextureTranscodeRequest,
        ) -> pkg2mpkg_core::Result<TextureTranscodeReport> {
            *self.calls.lock().unwrap() += 1;
            fs::remove_file(&self.source_alias).unwrap();
            fs::remove_file(&self.output_parent_alias).unwrap();
            std::os::unix::fs::symlink(&self.decoy_root, &self.source_alias).unwrap();
            std::os::unix::fs::symlink(&self.original_root, &self.output_parent_alias).unwrap();
            FakeOkBackend.transcode_texture(request)
        }
    }

    let backend = RetargetingBackend {
        source_alias,
        output_parent_alias,
        original_root: original_root.clone(),
        decoy_root,
        calls: Mutex::new(0),
    };
    let context = ExportContext::with_resource_backend(&backend);

    let error =
        execute_export_plan(&source, &plan, &context, OverwritePolicy::Replace).unwrap_err();

    assert_eq!(error.code(), ErrorCode::InvalidArguments);
    assert_eq!(*backend.calls.lock().unwrap(), 1);
    assert_eq!(
        fs::read(original_root.join("project.json")).unwrap(),
        original_manifest
    );
}

#[test]
fn replace_rejects_hardlink_alias_to_source_file_before_backend() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let source_tex = fixture.path().join("materials/opaque.tex");
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("source-alias.mpkg");
    fs::hard_link(&source_tex, &output).unwrap();
    let original = fs::read(&source_tex).unwrap();
    let plan = dynamic_plan(&source, output.clone());
    let backend = CountingOkBackend::new();
    let context = ExportContext::with_resource_backend(&backend);

    let error =
        execute_export_plan(&source, &plan, &context, OverwritePolicy::Replace).unwrap_err();

    assert_eq!(error.code(), ErrorCode::InvalidArguments);
    assert_eq!(backend.calls(), 0);
    assert_eq!(fs::read(&source_tex).unwrap(), original);
    assert_eq!(fs::read(&output).unwrap(), original);
}

#[test]
fn output_beside_scene_directory_remains_allowed() {
    let workspace = tempdir().unwrap();
    let project = workspace.path().join("project");
    write_bytes(
        &project.join("project.json"),
        br#"{"title":"Beside","type":"scene","file":"scene.json"}"#,
    );
    write_bytes(&project.join("scene.json"), br#"{"objects":[]}"#);
    let source = inspect_source(&project).unwrap();
    let output = workspace.path().join("mobile.mpkg");
    let plan = dynamic_plan(&source, output.clone());
    let backend = CountingOkBackend::new();
    let context = ExportContext::with_resource_backend(&backend);

    execute_export_plan(&source, &plan, &context, OverwritePolicy::Replace).unwrap();

    assert!(output.is_file());
    assert_eq!(backend.calls(), 0, "zero-TEX Scene must not call backend");
}

#[test]
fn web_and_application_are_unsupported() {
    for project in [synthetic_web_project(), synthetic_application_project()] {
        // inspect_source rejects these types; build a synthetic SourceProject.
        let kind = if project.path().join("index.html").exists() {
            WallpaperKind::Web
        } else {
            WallpaperKind::Application
        };
        let entry = if kind == WallpaperKind::Web {
            "index.html"
        } else {
            "demo.exe"
        };
        let manifest_bytes = format!(
            r#"{{"title":"Blocked","type":"{}","file":"{entry}"}}"#,
            kind.as_str()
        );
        let source = SourceProject {
            root: project.path().to_path_buf(),
            project_file: Some(project.path().join("project.json")),
            entry_file: project.path().join(entry),
            title: "Blocked".into(),
            kind,
            manifest: ProjectManifest::parse(manifest_bytes.as_bytes()).unwrap(),
        };
        let plan = pkg2mpkg_core::ExportPlan {
            source: source.root.clone(),
            title: source.title.clone(),
            kind,
            mode: ExportMode::SceneDynamic {
                compression: pkg2mpkg_core::Compression::HighPerformance,
                reduction: pkg2mpkg_core::Reduction::Original,
            },
            compatibility: pkg2mpkg_core::CompatibilityTarget::WeAndroid { major: 2, minor: 8 },
            properties: Default::default(),
            transformations: vec![],
            helpers: vec![HelperRequirement::ResourceTranscode],
            estimated_size: None,
            output: project.path().join("out.mpkg"),
        };
        let backend = FakeOkBackend;
        let context = ExportContext::with_resource_backend(&backend);
        let error =
            execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnsupportedWallpaperType);
        assert_eq!(error.code().exit_code(), 3);
        assert!(!plan.output.exists());
    }
}

#[test]
fn performance_pre_render_is_backend_unavailable() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let output = fixture.path().join("perf.mpkg");
    let plan = build_export_plan(
        &source,
        ExportRequest::scene(
            output.clone(),
            SceneProfile::Performance,
            ContentClass::Normal,
        ),
    )
    .unwrap();
    assert_eq!(plan.mode, ExportMode::ScenePreRenderedVideo);

    let backend = FakeOkBackend;
    let context = ExportContext::with_resource_backend(&backend);
    let error = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap_err();
    assert_eq!(error.code(), ErrorCode::BackendUnavailable);
    assert_eq!(error.code().exit_code(), 5);
    assert!(!output.exists());
}

#[test]
fn video_export_is_backend_unavailable() {
    let project = synthetic_video_project();
    let source = inspect_source(project.path()).unwrap();
    let output = project.path().join("video.mpkg");
    let plan = build_export_plan(
        &source,
        ExportRequest::video(output.clone(), VideoInputCompatibility::Unknown),
    )
    .unwrap();
    assert!(matches!(plan.mode, ExportMode::Video { .. }));

    let backend = FakeOkBackend;
    let context = ExportContext::with_resource_backend(&backend);
    let error = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap_err();
    assert_eq!(error.code(), ErrorCode::BackendUnavailable);
    assert_eq!(error.code().exit_code(), 5);
    assert!(!output.exists());
}

#[test]
fn source_plan_mismatch_is_invalid_arguments() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let mut plan = dynamic_plan(&source, fixture.path().join("out.mpkg"));
    plan.title = "Different Title".into();

    let backend = FakeOkBackend;
    let context = ExportContext::with_resource_backend(&backend);
    let error = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArguments);
    assert_eq!(error.code().exit_code(), 2);
    assert!(!plan.output.exists());
}

#[test]
fn loose_scene_swapped_for_packaged_sibling_is_source_plan_mismatch() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Swapped","type":"scene","file":"scene.json"}"#,
    );
    write_bytes(&dir.path().join("scene.json"), br#"{"objects":[]}"#);

    let source = inspect_source(dir.path()).unwrap();
    let output = dir.path().join("out.mpkg");
    let plan = dynamic_plan(&source, output.clone());

    fs::remove_file(dir.path().join("scene.json")).unwrap();
    write_bytes(&dir.path().join("scene.pkg"), b"PKGV0001-replacement");

    let backend = FakeOkBackend;
    let context = ExportContext::with_resource_backend(&backend);
    let error = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap_err();

    assert_eq!(error.code(), ErrorCode::InvalidArguments);
    assert!(
        error.to_string().contains("physical source entry changed"),
        "unexpected mismatch error: {error}"
    );
    assert!(!output.exists());
}

#[test]
fn packaged_scene_swapped_for_loose_sibling_is_source_plan_mismatch() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Swapped","type":"scene","file":"scene.json"}"#,
    );
    write_bytes(&dir.path().join("scene.pkg"), b"PKGV0001-original");

    let source = inspect_source(dir.path()).unwrap();
    let output = dir.path().join("out.mpkg");
    let plan = dynamic_plan(&source, output.clone());

    fs::remove_file(dir.path().join("scene.pkg")).unwrap();
    write_bytes(&dir.path().join("scene.json"), br#"{"objects":[]}"#);

    let backend = FakeOkBackend;
    let context = ExportContext::with_resource_backend(&backend);
    let error = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap_err();

    assert_eq!(error.code(), ErrorCode::InvalidArguments);
    assert!(
        error.to_string().contains("physical source entry changed"),
        "unexpected mismatch error: {error}"
    );
    assert!(!output.exists());
}

#[test]
fn mismatched_plan_root_is_invalid_arguments() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let mut plan = dynamic_plan(&source, fixture.path().join("out.mpkg"));
    plan.source = PathBuf::from("/tmp/definitely-not-this-project");

    let backend = FakeOkBackend;
    let context = ExportContext::with_resource_backend(&backend);
    let error = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArguments);
    assert!(!plan.output.exists());
}

#[test]
fn missing_resource_backend_is_backend_unavailable() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");
    let plan = dynamic_plan(&source, output.clone());

    let error = execute_export_plan(&source, &plan, &ExportContext::new(), OverwritePolicy::Deny)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::BackendUnavailable);
    assert_eq!(error.code().exit_code(), 5);
    assert!(!output.exists());
}

#[test]
fn backend_conversion_failure_leaves_no_output() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");
    let plan = dynamic_plan(&source, output.clone());

    let backend = FailingBackend;
    let context = ExportContext::with_resource_backend(&backend);
    let error = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ConversionFailed);
    assert_eq!(error.code().exit_code(), 6);
    assert!(!output.exists());
    assert!(partials_in(out_dir.path()).is_empty());
}

#[test]
fn invalid_tex_from_successful_backend_fails_before_publish() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");
    let plan = dynamic_plan(&source, output.clone());

    let backend = InvalidTexBackend;
    let context = ExportContext::with_resource_backend(&backend);
    let error = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap_err();
    assert_eq!(error.code(), ErrorCode::VerificationFailed);
    assert_eq!(error.code().exit_code(), 8);
    assert!(!output.exists());
    assert!(partials_in(out_dir.path()).is_empty());
}

#[test]
fn deny_preserves_competitor_output_file() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");
    fs::write(&output, b"competitor-bytes").unwrap();
    let plan = dynamic_plan(&source, output.clone());

    let backend = FakeOkBackend;
    let context = ExportContext::with_resource_backend(&backend);
    let error = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap_err();
    assert_eq!(error.code(), ErrorCode::OutputIo);
    assert_eq!(fs::read(&output).unwrap(), b"competitor-bytes");
    assert!(partials_in(out_dir.path()).is_empty());
}

#[test]
fn replace_conversion_failure_preserves_old_output() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");
    fs::write(&output, b"precious-old-package").unwrap();
    let plan = dynamic_plan(&source, output.clone());

    let backend = FailingBackend;
    let context = ExportContext::with_resource_backend(&backend);
    let error =
        execute_export_plan(&source, &plan, &context, OverwritePolicy::Replace).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ConversionFailed);
    assert_eq!(fs::read(&output).unwrap(), b"precious-old-package");
    assert!(partials_in(out_dir.path()).is_empty());
}

#[test]
fn replace_verification_failure_preserves_old_output() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");
    fs::write(&output, b"precious-old-package").unwrap();
    let plan = dynamic_plan(&source, output.clone());

    let backend = InvalidTexBackend;
    let context = ExportContext::with_resource_backend(&backend);
    let error =
        execute_export_plan(&source, &plan, &context, OverwritePolicy::Replace).unwrap_err();
    assert_eq!(error.code(), ErrorCode::VerificationFailed);
    assert_eq!(fs::read(&output).unwrap(), b"precious-old-package");
    assert!(partials_in(out_dir.path()).is_empty());
}

#[test]
fn raw_scene_pkg_entry_is_rejected_until_unpack_backend() {
    let dir = tempdir().unwrap();
    write_bytes(&dir.path().join("scene.pkg"), b"PKGV0001-fake-scene-pkg");
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Packaged","type":"scene","file":"scene.pkg"}"#,
    );
    let source = inspect_source(dir.path()).unwrap();
    assert_eq!(source.kind, WallpaperKind::Scene);
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");
    let plan = dynamic_plan(&source, output.clone());

    // Resource backend alone is not enough: packaged entry needs scene_pkg_unpack.
    let backend = FakeOkBackend;
    let context = ExportContext::with_resource_backend(&backend);
    let error = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap_err();
    assert_eq!(error.code(), ErrorCode::BackendUnavailable);
    assert!(
        error.to_string().contains("scene_pkg_unpack"),
        "error message must name scene_pkg_unpack backend, got: {error}"
    );
    assert!(!output.exists());
}

#[test]
fn failures_do_not_leave_task_debris_beside_output() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");
    let plan = dynamic_plan(&source, output.clone());

    let backend = FailingBackend;
    let context = ExportContext::with_resource_backend(&backend);
    let _ = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap_err();

    let leftover: Vec<_> = fs::read_dir(out_dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        leftover.is_empty(),
        "output directory must be clean after failure, found {leftover:?}"
    );
}

#[test]
fn deny_existing_does_not_invoke_backend() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");
    fs::write(&output, b"already-there").unwrap();
    let plan = dynamic_plan(&source, output.clone());

    struct CountingBackend {
        calls: Mutex<u32>,
    }
    impl ResourceTranscodeBackend for CountingBackend {
        fn transcode_texture(
            &self,
            request: &TextureTranscodeRequest,
        ) -> pkg2mpkg_core::Result<TextureTranscodeReport> {
            *self.calls.lock().unwrap() += 1;
            FakeOkBackend.transcode_texture(request)
        }
    }

    let backend = CountingBackend {
        calls: Mutex::new(0),
    };
    let context = ExportContext::with_resource_backend(&backend);
    let error = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap_err();
    assert_eq!(error.code(), ErrorCode::OutputIo);
    assert_eq!(*backend.calls.lock().unwrap(), 0);
    assert_eq!(fs::read(&output).unwrap(), b"already-there");
}
