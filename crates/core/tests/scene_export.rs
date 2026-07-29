//! Happy-path end-to-end dynamic Scene export tests (Task 5).

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use pkg2mpkg_core::{
    Compression, ContainerVersion, ContentClass, ErrorCode, ExportContext, ExportMode,
    ExportRequest, MpkgArchive, NativeScenePackageUnpackBackend, OverwritePolicy, Reduction,
    ResourceTranscodeBackend, ScenePackageEntry, ScenePackageUnpackBackend,
    ScenePackageUnpackReport, ScenePackageUnpackRequest, SceneProfile, Stage,
    TextureTranscodeReport, TextureTranscodeRequest, WallpaperKind, build_export_plan,
    build_mobile_scene_project_json, execute_export_plan, inspect_source,
    package_entry_is_raw_scene_pkg, source_requires_package_unpack,
};
use pkg2mpkg_fixtures::{dynamic_scene_project, raw_pkg, snapshot_tree, write_bytes};
use tempfile::tempdir;

const TEX_V5_MAGIC: &[u8; 9] = b"TEXV0005\0";

/// In-process fake that records every request and writes a valid TEXV0005 body.
#[derive(Default)]
struct FakeTranscodeBackend {
    calls: Mutex<Vec<TextureTranscodeRequest>>,
    /// Optional: rewrite output bytes after writing the default payload.
    payload_suffix: Vec<u8>,
}

impl FakeTranscodeBackend {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            payload_suffix: b"converted-by-fake".to_vec(),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    fn calls(&self) -> Vec<TextureTranscodeRequest> {
        self.calls.lock().unwrap().clone()
    }
}

impl ResourceTranscodeBackend for FakeTranscodeBackend {
    fn transcode_texture(
        &self,
        request: &TextureTranscodeRequest,
    ) -> pkg2mpkg_core::Result<TextureTranscodeReport> {
        self.calls.lock().unwrap().push(request.clone());
        let input_bytes = fs::metadata(&request.input)
            .map(|meta| meta.len())
            .unwrap_or(0);
        let mut body = TEX_V5_MAGIC.to_vec();
        body.extend_from_slice(&self.payload_suffix);
        if let Some(parent) = request.output.parent() {
            fs::create_dir_all(parent).map_err(|source| pkg2mpkg_core::Error::Io {
                stage: pkg2mpkg_core::Stage::Convert,
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(&request.output, &body).map_err(|source| pkg2mpkg_core::Error::Io {
            stage: pkg2mpkg_core::Stage::Convert,
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

fn plan_for(source: &pkg2mpkg_core::SourceProject, output: PathBuf) -> pkg2mpkg_core::ExportPlan {
    build_export_plan(
        source,
        ExportRequest::scene(output, SceneProfile::High, ContentClass::Normal),
    )
    .unwrap()
}

fn archive_paths(archive: &MpkgArchive) -> Vec<String> {
    archive
        .entries()
        .iter()
        .map(|entry| entry.path.clone())
        .collect()
}

#[test]
fn dynamic_scene_exports_once_per_tex_with_ordered_pkgm0020() {
    let fixture = dynamic_scene_project();
    let before = snapshot_tree(fixture.path());
    let source = inspect_source(fixture.path()).unwrap();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("dynamic.mpkg");

    let plan = plan_for(&source, output.clone());
    assert!(matches!(
        plan.mode,
        ExportMode::SceneDynamic {
            compression: Compression::HighPerformance,
            reduction: Reduction::Original,
        }
    ));

    let backend = FakeTranscodeBackend::new();
    let context = ExportContext::with_resource_backend(&backend);
    let report = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap();

    // Exactly one conversion for the single .tex in the fixture.
    assert_eq!(backend.call_count(), 1);
    let calls = backend.calls();
    assert_eq!(calls[0].compression, Compression::HighPerformance);
    assert_eq!(calls[0].reduction, Reduction::Original);
    assert_eq!(calls[0].max_mipmaps, 1);
    assert!(
        calls[0].input.ends_with(Path::new("materials/opaque.tex"))
            || calls[0]
                .input
                .file_name()
                .is_some_and(|name| name == "opaque.tex"),
        "conversion input should be the snapshot tex, got {:?}",
        calls[0].input
    );

    // Deterministic report without temp/timing noise.
    assert_eq!(report.kind, WallpaperKind::Scene);
    assert_eq!(
        report.mode,
        ExportMode::SceneDynamic {
            compression: Compression::HighPerformance,
            reduction: Reduction::Original,
        }
    );
    assert_eq!(report.container_version, ContainerVersion::Pkgm0020);
    assert_eq!(report.output, output);
    assert_eq!(report.texture_count, 1);
    assert!(report.texture_input_bytes > 0);
    assert!(report.texture_output_bytes > 0);
    assert!(report.entry_count > 0);
    assert!(report.output_bytes > 0);
    let report_json = serde_json::to_value(&report).unwrap();
    for forbidden in [
        "elapsed",
        "duration",
        "timestamp",
        "temp",
        "pid",
        "process",
        "diagnostic",
    ] {
        let text = report_json.to_string().to_ascii_lowercase();
        assert!(
            !text.contains(forbidden),
            "ExportReport JSON must not contain {forbidden}: {text}"
        );
    }

    let archive = MpkgArchive::open(&output).unwrap();
    assert_eq!(archive.version(), ContainerVersion::Pkgm0020);

    let paths = archive_paths(&archive);
    let mut sorted = paths.clone();
    sorted.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    assert_eq!(paths, sorted, "archive entries must be bytewise ordered");

    for required in [
        "project.json",
        "scene.json",
        "preview.jpg",
        "materials/main.json",
        "materials/opaque.tex",
        "shaders/effects/pulse.frag",
        "sounds/click.wav",
        "nested/deep/note.txt",
        "unused/extra.bin",
        ".keep-dotfile",
    ] {
        assert!(
            paths.iter().any(|path| path == required),
            "missing required archive path {required} in {paths:?}"
        );
    }
    for forbidden in [
        "materials/opaque.tex-json",
        "materials/opaque.TEX-JSON",
        "export.mpkg",
        "stage.partial",
        ".pkg2mpkg-debris/tmp.bin",
    ] {
        assert!(
            !paths
                .iter()
                .any(|path| path.eq_ignore_ascii_case(forbidden)),
            "forbidden path leaked into package: {forbidden}"
        );
    }

    // Non-TEX files are byte-preserved from the source snapshot.
    assert_eq!(
        archive.read_entry("scene.json").unwrap(),
        fs::read(fixture.path().join("scene.json")).unwrap()
    );
    assert_eq!(
        archive.read_entry("preview.jpg").unwrap(),
        fs::read(fixture.path().join("preview.jpg")).unwrap()
    );
    assert_eq!(
        archive.read_entry("materials/main.json").unwrap(),
        fs::read(fixture.path().join("materials/main.json")).unwrap()
    );
    assert_eq!(
        archive.read_entry("nested/deep/note.txt").unwrap(),
        fs::read(fixture.path().join("nested/deep/note.txt")).unwrap()
    );

    // project.json is the Task 4 mobile construction, not the raw source bytes.
    let mobile = build_mobile_scene_project_json(&source, &plan).unwrap();
    assert_eq!(archive.read_entry("project.json").unwrap(), mobile);
    assert_ne!(
        archive.read_entry("project.json").unwrap(),
        fs::read(fixture.path().join("project.json")).unwrap()
    );

    // Converted TEX must be valid TEXV0005.
    let tex = archive.read_entry("materials/opaque.tex").unwrap();
    assert!(
        tex.starts_with(TEX_V5_MAGIC),
        "converted tex must start with TEXV0005\\0"
    );
    assert_ne!(
        tex,
        fs::read(fixture.path().join("materials/opaque.tex")).unwrap()
    );

    assert_eq!(report.entry_count, paths.len());
    assert_eq!(snapshot_tree(fixture.path()), before, "source tree mutated");
    assert!(
        fs::read_dir(out_dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name == "dynamic.mpkg" || !name.contains("partial")
            }),
        "no partial debris beside output"
    );
}

#[test]
fn mixed_case_tex_extension_is_converted_exactly_once() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Case Tex","type":"scene","file":"scene.json"}"#,
    );
    write_bytes(&dir.path().join("Alpha.TEX"), b"TEXV0005\0desktop-alpha");
    write_bytes(&dir.path().join("beta.TeX"), b"TEXV0005\0desktop-beta");
    write_bytes(&dir.path().join("readme.txt"), b"not a texture");

    let source = inspect_source(dir.path()).unwrap();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");
    let plan = plan_for(&source, output.clone());
    let backend = FakeTranscodeBackend::new();
    let context = ExportContext::with_resource_backend(&backend);

    let report = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap();
    assert_eq!(backend.call_count(), 2);
    assert_eq!(report.texture_count, 2);

    let archive = MpkgArchive::open(&output).unwrap();
    let paths = archive_paths(&archive);
    assert!(paths.iter().any(|path| path == "Alpha.TEX"));
    assert!(paths.iter().any(|path| path == "beta.TeX"));
    assert!(
        archive
            .read_entry("Alpha.TEX")
            .unwrap()
            .starts_with(TEX_V5_MAGIC)
    );
    assert!(
        archive
            .read_entry("beta.TeX")
            .unwrap()
            .starts_with(TEX_V5_MAGIC)
    );
    assert_eq!(archive.read_entry("readme.txt").unwrap(), b"not a texture");
}

#[test]
fn zero_tex_project_still_requires_backend_and_packages() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"No Tex","type":"scene","file":"scene.json"}"#,
    );
    write_bytes(&dir.path().join("note.txt"), b"plain");

    let source = inspect_source(dir.path()).unwrap();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");
    let plan = plan_for(&source, output.clone());

    let missing = execute_export_plan(&source, &plan, &ExportContext::new(), OverwritePolicy::Deny)
        .unwrap_err();
    assert_eq!(missing.code(), ErrorCode::BackendUnavailable);
    assert!(!output.exists());

    let backend = FakeTranscodeBackend::new();
    let context = ExportContext::with_resource_backend(&backend);
    let report = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap();
    assert_eq!(backend.call_count(), 0);
    assert_eq!(report.texture_count, 0);
    assert!(MpkgArchive::open(&output).is_ok());
}

#[test]
fn replace_overwrites_existing_output_only_after_success() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("dynamic.mpkg");
    fs::write(&output, b"stale-placeholder").unwrap();

    let plan = plan_for(&source, output.clone());
    let backend = FakeTranscodeBackend::new();
    let context = ExportContext::with_resource_backend(&backend);
    let report = execute_export_plan(&source, &plan, &context, OverwritePolicy::Replace).unwrap();

    assert_eq!(report.output, output);
    assert_ne!(fs::read(&output).unwrap(), b"stale-placeholder");
    let archive = MpkgArchive::open(&output).unwrap();
    assert_eq!(archive.version(), ContainerVersion::Pkgm0020);
    assert!(archive.read_entry("project.json").is_ok());
}

#[test]
fn post_snapshot_source_mutation_does_not_change_package() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Mutable","type":"scene","file":"scene.json"}"#,
    );
    write_bytes(
        &dir.path().join("materials/opaque.tex"),
        b"TEXV0005\0original",
    );
    write_bytes(&dir.path().join("note.txt"), b"original-note");

    let source = inspect_source(dir.path()).unwrap();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");
    let plan = plan_for(&source, output.clone());

    // Backend mutates the live source after it has been asked to convert — the
    // package must still reflect the pre-mutation snapshot.
    struct MutatingBackend {
        root: PathBuf,
        inner: FakeTranscodeBackend,
    }
    impl ResourceTranscodeBackend for MutatingBackend {
        fn transcode_texture(
            &self,
            request: &TextureTranscodeRequest,
        ) -> pkg2mpkg_core::Result<TextureTranscodeReport> {
            write_bytes(
                &self.root.join("materials/opaque.tex"),
                b"TEXV0005\0MUTATED-AFTER-SNAPSHOT",
            );
            write_bytes(&self.root.join("note.txt"), b"mutated-note");
            self.inner.transcode_texture(request)
        }
    }

    let backend = MutatingBackend {
        root: dir.path().to_path_buf(),
        inner: FakeTranscodeBackend::new(),
    };
    let context = ExportContext::with_resource_backend(&backend);
    execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap();

    let archive = MpkgArchive::open(&output).unwrap();
    assert_eq!(archive.read_entry("note.txt").unwrap(), b"original-note");
    // Converted TEX comes from the snapshot input (original), not the mutated source.
    let tex = archive.read_entry("materials/opaque.tex").unwrap();
    assert!(tex.starts_with(TEX_V5_MAGIC));
    assert!(!tex.windows(b"MUTATED".len()).any(|w| w == b"MUTATED"));
}

/// Hermetic fake unpack backend (mirrors `scene_pkg_input` test helper).
struct FakeUnpackBackend {
    packages: Mutex<BTreeMap<PathBuf, BTreeMap<String, Vec<u8>>>>,
}

impl FakeUnpackBackend {
    fn new() -> Self {
        Self {
            packages: Mutex::new(BTreeMap::new()),
        }
    }

    fn register(&self, package: &Path, entries: BTreeMap<String, Vec<u8>>) {
        self.packages
            .lock()
            .unwrap()
            .insert(package.to_path_buf(), entries);
    }
}

impl ScenePackageUnpackBackend for FakeUnpackBackend {
    fn unpack_scene_package(
        &self,
        request: &ScenePackageUnpackRequest,
    ) -> pkg2mpkg_core::Result<ScenePackageUnpackReport> {
        let packages = self.packages.lock().unwrap();
        let entries = packages.get(&request.package).ok_or_else(|| {
            pkg2mpkg_core::Error::BackendUnavailable {
                backend: format!("fake scene package {}", request.package.display()),
            }
        })?;

        fs::create_dir_all(&request.output_dir).map_err(|source| pkg2mpkg_core::Error::Io {
            stage: Stage::Unpack,
            path: request.output_dir.clone(),
            source,
        })?;

        let mut report_entries = Vec::new();
        let mut total_bytes = 0_u64;
        for (path, bytes) in entries {
            if path.is_empty()
                || path.contains('\0')
                || path.contains('\\')
                || path.starts_with('/')
                || path
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
            {
                return Err(pkg2mpkg_core::Error::InvalidProject {
                    reason: format!("unsafe unpack archive path: {path:?}"),
                });
            }
            let size = bytes.len() as u64;
            total_bytes = total_bytes.saturating_add(size);
            let dest = {
                let mut dest = request.output_dir.clone();
                for part in path.split('/') {
                    dest.push(part);
                }
                dest
            };
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|source| pkg2mpkg_core::Error::Io {
                    stage: Stage::Unpack,
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            fs::write(&dest, bytes).map_err(|source| pkg2mpkg_core::Error::Io {
                stage: Stage::Unpack,
                path: dest.clone(),
                source,
            })?;
            report_entries.push(ScenePackageEntry {
                path: path.clone(),
                size,
            });
        }

        Ok(ScenePackageUnpackReport {
            output_dir: request.output_dir.clone(),
            entries: report_entries,
            total_bytes,
        })
    }
}

#[test]
fn packaged_scene_exports_via_unpack_backend_without_pkg_in_mpkg() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Packaged Export","type":"scene","file":"scene.pkg","preview":"preview.jpg"}"#,
    );
    write_bytes(&dir.path().join("preview.jpg"), b"JPEG-PREVIEW");
    let package = dir.path().join("scene.pkg");
    write_bytes(&package, b"PKGV0001-opaque-desktop-pkg");

    let scene_json = br#"{"general":{},"objects":[]}"#;
    let material_json = br#"{"passes":[]}"#;
    let tex_payload = b"TEXV0005\0desktop-packaged-tex";
    let unpack_entries = BTreeMap::from([
        ("scene.json".into(), scene_json.to_vec()),
        ("materials/main.json".into(), material_json.to_vec()),
        ("materials/x.tex".into(), tex_payload.to_vec()),
        ("shaders/fx.frag".into(), b"FRAG".to_vec()),
    ]);

    let source = inspect_source(dir.path()).unwrap();
    assert_eq!(source.kind, WallpaperKind::Scene);
    assert!(package_entry_is_raw_scene_pkg(
        source.manifest.entry().unwrap()
    ));

    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("packaged.mpkg");
    let plan = plan_for(&source, output.clone());

    let unpack = FakeUnpackBackend::new();
    unpack.register(&package, unpack_entries);
    let tex_backend = FakeTranscodeBackend::new();
    let context =
        ExportContext::with_resource_backend(&tex_backend).package_unpack_backend(&unpack);

    let report = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap();
    assert_eq!(report.container_version, ContainerVersion::Pkgm0020);
    assert_eq!(report.kind, WallpaperKind::Scene);
    assert_eq!(report.texture_count, 1);
    assert_eq!(tex_backend.call_count(), 1);
    assert!(report.output_bytes > 0);

    let archive = MpkgArchive::open(&output).unwrap();
    assert_eq!(archive.version(), ContainerVersion::Pkgm0020);
    let paths = archive_paths(&archive);

    assert!(paths.iter().any(|p| p == "project.json"));
    assert!(paths.iter().any(|p| p == "scene.json"));
    assert!(paths.iter().any(|p| p == "preview.jpg"));
    assert!(paths.iter().any(|p| p == "materials/main.json"));
    assert!(paths.iter().any(|p| p == "materials/x.tex"));
    assert!(paths.iter().any(|p| p == "shaders/fx.frag"));
    assert!(
        !paths
            .iter()
            .any(|p| package_entry_is_raw_scene_pkg(p) || p.eq_ignore_ascii_case("scene.pkg")),
        "raw .pkg must not appear in final MPKG: {paths:?}"
    );

    // Loose scene payload preserved; TEX converted to TEXV0005.
    assert_eq!(archive.read_entry("scene.json").unwrap(), scene_json);
    assert_eq!(
        archive.read_entry("materials/main.json").unwrap(),
        material_json
    );
    assert_eq!(archive.read_entry("shaders/fx.frag").unwrap(), b"FRAG");
    assert_eq!(archive.read_entry("preview.jpg").unwrap(), b"JPEG-PREVIEW");
    let tex = archive.read_entry("materials/x.tex").unwrap();
    assert!(
        tex.starts_with(TEX_V5_MAGIC),
        "converted tex must start with TEXV0005\\0"
    );
    assert_ne!(tex, tex_payload);

    // Mobile project.json must reference the loose scene entry, not scene.pkg.
    let project = archive.read_entry("project.json").unwrap();
    let project_text = String::from_utf8(project).unwrap();
    assert!(
        project_text.contains("scene.json"),
        "mobile project.json should point at loose scene: {project_text}"
    );
}

#[test]
fn packaged_scene_exports_via_native_pkgv_unpack_without_pkg_in_mpkg() {
    assert_native_packaged_scene_exports("scene.pkg");
}

#[test]
fn workshop_scene_json_manifest_exports_via_sibling_native_pkgv() {
    assert_native_packaged_scene_exports("scene.json");
}

fn assert_native_packaged_scene_exports(manifest_entry: &str) {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("project.json"),
        format!(
            r#"{{"title":"Native Packaged","type":"scene","file":"{manifest_entry}","preview":"preview.jpg"}}"#
        )
        .as_bytes(),
    );
    write_bytes(&dir.path().join("preview.jpg"), b"JPEG-PREVIEW");
    let package = dir.path().join("scene.pkg");

    let scene_json = br#"{"general":{},"objects":[]}"#;
    let material_json = br#"{"passes":[]}"#;
    let tex_payload = b"TEXV0005\0desktop-packaged-tex";
    fs::write(
        &package,
        raw_pkg(
            "PKGV0001",
            &[
                ("scene.json", scene_json.as_slice()),
                ("materials/main.json", material_json.as_slice()),
                ("materials/x.tex", tex_payload.as_slice()),
                ("shaders/fx.frag", b"FRAG"),
            ],
        ),
    )
    .unwrap();

    let source = inspect_source(dir.path()).unwrap();
    assert!(source_requires_package_unpack(&source));

    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("native-packaged.mpkg");
    let plan = plan_for(&source, output.clone());

    let tex_backend = FakeTranscodeBackend::new();
    let unpack = NativeScenePackageUnpackBackend::new();
    let context =
        ExportContext::with_resource_backend(&tex_backend).package_unpack_backend(&unpack);

    let report = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap();
    assert_eq!(report.container_version, ContainerVersion::Pkgm0020);
    assert_eq!(report.texture_count, 1);
    assert_eq!(tex_backend.call_count(), 1);

    let archive = MpkgArchive::open(&output).unwrap();
    let paths = archive_paths(&archive);
    assert!(paths.iter().any(|p| p == "scene.json"));
    assert!(paths.iter().any(|p| p == "materials/x.tex"));
    assert!(
        !paths
            .iter()
            .any(|p| package_entry_is_raw_scene_pkg(p) || p.eq_ignore_ascii_case("scene.pkg")),
        "raw .pkg must not appear in final MPKG: {paths:?}"
    );
    assert_eq!(archive.read_entry("scene.json").unwrap(), scene_json);
    let tex = archive.read_entry("materials/x.tex").unwrap();
    assert!(tex.starts_with(TEX_V5_MAGIC));
    assert_ne!(tex, tex_payload);
    let project = archive.read_entry("project.json").unwrap();
    let project_text = String::from_utf8(project).unwrap();
    assert!(
        project_text.contains("scene.json"),
        "mobile project.json should point at loose scene: {project_text}"
    );
    assert!(
        !project_text.to_ascii_lowercase().contains("scene.pkg"),
        "mobile project.json must not reference scene.pkg: {project_text}"
    );
}

#[test]
fn export_report_is_byte_stable_for_identical_inputs() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();
    let out_dir = tempdir().unwrap();

    let backend = FakeTranscodeBackend::new();
    let context = ExportContext::with_resource_backend(&backend);

    let output_a = out_dir.path().join("a.mpkg");
    let plan_a = plan_for(&source, output_a.clone());
    let report_a = execute_export_plan(&source, &plan_a, &context, OverwritePolicy::Deny).unwrap();

    let output_b = out_dir.path().join("b.mpkg");
    let plan_b = plan_for(&source, output_b.clone());
    let report_b = execute_export_plan(&source, &plan_b, &context, OverwritePolicy::Deny).unwrap();

    // Strip output path differences: serialize after normalizing output field.
    let mut value_a = serde_json::to_value(&report_a).unwrap();
    let mut value_b = serde_json::to_value(&report_b).unwrap();
    value_a.as_object_mut().unwrap().remove("output");
    value_b.as_object_mut().unwrap().remove("output");
    assert_eq!(value_a, value_b);
}

/// Plant a synthetic zcompat rule directory: `<root>/<project-id>/config.json` + shaders.
fn write_zcompat_rule(
    zcompat_root: &Path,
    project_id: &str,
    maximum_project_id: &str,
    frag: &str,
    vert: &str,
    frag_body: &[u8],
    vert_body: &[u8],
) {
    let rule_dir = zcompat_root.join(project_id);
    fs::create_dir_all(&rule_dir).unwrap();
    write_bytes(
        &rule_dir.join("config.json"),
        format!(r#"{{"maximumprojectid":"{maximum_project_id}","frag":"{frag}","vert":"{vert}"}}"#)
            .as_bytes(),
    );
    write_bytes(&rule_dir.join(frag), frag_body);
    write_bytes(&rule_dir.join(vert), vert_body);
}

#[test]
fn export_applies_compat_shaders_on_snapshot_not_source() {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    let zcompat = root.path().join("zcompat");
    fs::create_dir_all(project.join("shaders/effects")).unwrap();
    fs::create_dir_all(project.join("materials")).unwrap();

    let original_frag = b"// ORIGINAL-FRAG\nvoid main() {}\n";
    let original_vert = b"// ORIGINAL-VERT\nvoid main() {}\n";
    let compat_frag = b"// COMPAT-FRAG-OVERRIDE\n";
    let compat_vert = b"// COMPAT-VERT-OVERRIDE\n";

    write_bytes(
        &project.join("project.json"),
        br#"{"title":"Compat Export","type":"scene","file":"scene.json","workshopid":"2078835426"}"#,
    );
    write_bytes(
        &project.join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(
        &project.join("materials/opaque.tex"),
        b"TEXV0005\0desktop-tex",
    );
    write_bytes(&project.join("shaders/effects/pulse.frag"), original_frag);
    write_bytes(&project.join("shaders/effects/pulse.vert"), original_vert);
    write_bytes(&project.join("shaders/effects/other.frag"), b"OTHER-FRAG");

    write_zcompat_rule(
        &zcompat,
        "2078835426",
        "9223372036854775807",
        "pulse.frag",
        "pulse.vert",
        compat_frag,
        compat_vert,
    );

    let before = snapshot_tree(&project);
    let source = inspect_source(&project).unwrap();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("compat.mpkg");
    let plan = plan_for(&source, output.clone());

    let backend = FakeTranscodeBackend::new();
    let context = ExportContext::with_resource_backend(&backend).compat_shader_root(&zcompat);
    let report = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap();
    assert_eq!(report.container_version, ContainerVersion::Pkgm0020);
    assert_eq!(report.texture_count, 1);

    let archive = MpkgArchive::open(&output).unwrap();
    assert_eq!(
        archive.read_entry("shaders/effects/pulse.frag").unwrap(),
        compat_frag,
        "package must carry zcompat-replaced fragment shader"
    );
    assert_eq!(
        archive.read_entry("shaders/effects/pulse.vert").unwrap(),
        compat_vert,
        "package must carry zcompat-replaced vertex shader"
    );
    assert_eq!(
        archive.read_entry("shaders/effects/other.frag").unwrap(),
        b"OTHER-FRAG",
        "unrelated shaders must be byte-preserved"
    );

    // User's original source tree must never be mutated by export.
    assert_eq!(
        fs::read(project.join("shaders/effects/pulse.frag")).unwrap(),
        original_frag
    );
    assert_eq!(
        fs::read(project.join("shaders/effects/pulse.vert")).unwrap(),
        original_vert
    );
    assert_eq!(snapshot_tree(&project), before, "source tree mutated");
}

#[test]
fn export_compat_project_id_override_without_manifest_workshopid() {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    let zcompat = root.path().join("zcompat");
    fs::create_dir_all(project.join("shaders")).unwrap();

    write_bytes(
        &project.join("project.json"),
        br#"{"title":"No Workshop","type":"scene","file":"scene.json"}"#,
    );
    write_bytes(
        &project.join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(&project.join("shaders/fx.frag"), b"ORIG-F");
    write_bytes(&project.join("shaders/fx.vert"), b"ORIG-V");

    write_zcompat_rule(
        &zcompat,
        "42",
        "100",
        "fx.frag",
        "fx.vert",
        b"OVERRIDE-F",
        b"OVERRIDE-V",
    );

    let before = snapshot_tree(&project);
    let source = inspect_source(&project).unwrap();
    let output = root.path().join("out.mpkg");
    let plan = plan_for(&source, output.clone());

    let backend = FakeTranscodeBackend::new();
    let context = ExportContext::with_resource_backend(&backend)
        .compat_shader_root(&zcompat)
        .project_id_override("42");
    execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap();

    let archive = MpkgArchive::open(&output).unwrap();
    assert_eq!(
        archive.read_entry("shaders/fx.frag").unwrap(),
        b"OVERRIDE-F"
    );
    assert_eq!(
        archive.read_entry("shaders/fx.vert").unwrap(),
        b"OVERRIDE-V"
    );
    assert_eq!(
        fs::read(project.join("shaders/fx.frag")).unwrap(),
        b"ORIG-F"
    );
    assert_eq!(
        fs::read(project.join("shaders/fx.vert")).unwrap(),
        b"ORIG-V"
    );
    assert_eq!(snapshot_tree(&project), before);
}

#[test]
fn export_with_compat_root_but_no_project_id_is_no_op() {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    let zcompat = root.path().join("zcompat");
    fs::create_dir_all(project.join("shaders")).unwrap();

    write_bytes(
        &project.join("project.json"),
        br#"{"title":"No Id","type":"scene","file":"scene.json"}"#,
    );
    write_bytes(
        &project.join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(&project.join("shaders/fx.frag"), b"KEEP-ME");
    write_bytes(&project.join("shaders/fx.vert"), b"KEEP-V");

    // Rule exists but export has no workshopid and no override → no-op.
    write_zcompat_rule(
        &zcompat,
        "1",
        "999",
        "fx.frag",
        "fx.vert",
        b"SHOULD-NOT-APPLY",
        b"SHOULD-NOT-APPLY-V",
    );

    let source = inspect_source(&project).unwrap();
    let output = root.path().join("out.mpkg");
    let plan = plan_for(&source, output.clone());
    let backend = FakeTranscodeBackend::new();
    let context = ExportContext::with_resource_backend(&backend).compat_shader_root(&zcompat);
    execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny).unwrap();

    let archive = MpkgArchive::open(&output).unwrap();
    assert_eq!(archive.read_entry("shaders/fx.frag").unwrap(), b"KEEP-ME");
    assert_eq!(archive.read_entry("shaders/fx.vert").unwrap(), b"KEEP-V");
}
