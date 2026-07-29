use std::{fs, path::PathBuf};

use pkg2mpkg_core::{
    BackendCapabilities, ContentClass, ErrorCode, ExportMode, ExportRequest, HelperRequirement,
    ProjectManifest, SceneProfile, SourceProject, Transformation, VideoInputCompatibility,
    WallpaperKind, build_export_plan, inspect_source,
};
use tempfile::{TempDir, tempdir};

fn synthetic_scene_source() -> (TempDir, SourceProject) {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("scene.json"),
        br#"{"camera":{},"objects":[]}"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("project.json"),
        br#"{"title":"Fixture","type":"scene","file":"scene.json","general":{"properties":{"rate":{"value":2},"speed":{"value":1}}}}"#,
    )
    .unwrap();
    let source = inspect_source(dir.path()).unwrap();
    (dir, source)
}

fn blocked_source(kind: WallpaperKind, entry: &str) -> SourceProject {
    let manifest_bytes = format!(
        r#"{{"title":"Blocked","type":"{}","file":"{entry}"}}"#,
        kind.as_str(),
    );
    SourceProject {
        root: PathBuf::from("fixture"),
        project_file: Some(PathBuf::from("fixture/project.json")),
        entry_file: PathBuf::from(entry),
        title: "Blocked".into(),
        kind,
        manifest: ProjectManifest::parse(manifest_bytes.as_bytes()).unwrap(),
    }
}

#[test]
fn balanced_scene_plan_requires_resource_transcoding_not_scene_capture() {
    let (_dir, source) = synthetic_scene_source();
    let plan = build_export_plan(
        &source,
        ExportRequest::scene(
            PathBuf::from("balanced.mpkg"),
            SceneProfile::Balanced,
            ContentClass::Normal,
        ),
    )
    .unwrap();
    assert_eq!(plan.kind, WallpaperKind::Scene);
    assert!(matches!(plan.mode, ExportMode::SceneDynamic { .. }));
    assert_eq!(plan.helpers, vec![HelperRequirement::ResourceTranscode]);
    assert_eq!(plan.properties.len(), 1);
    assert_eq!(plan.properties["speed"]["value"], 1);
}

#[test]
fn performance_scene_plan_requires_capture_and_h264() {
    let (_dir, source) = synthetic_scene_source();
    let plan = build_export_plan(
        &source,
        ExportRequest::scene(
            PathBuf::from("performance.mpkg"),
            SceneProfile::Performance,
            ContentClass::Normal,
        ),
    )
    .unwrap();
    assert_eq!(plan.mode, ExportMode::ScenePreRenderedVideo);
    assert_eq!(
        plan.helpers,
        vec![
            HelperRequirement::SceneCapture,
            HelperRequirement::H264Encode
        ]
    );
}

#[test]
fn plan_json_is_byte_stable() {
    let (_dir, source) = synthetic_scene_source();
    let request = ExportRequest::scene(
        PathBuf::from("stable.mpkg"),
        SceneProfile::High,
        ContentClass::PixelArt,
    );
    let first =
        serde_json::to_vec_pretty(&build_export_plan(&source, request.clone()).unwrap()).unwrap();
    let second = serde_json::to_vec_pretty(&build_export_plan(&source, request).unwrap()).unwrap();
    assert_eq!(first, second);
    assert!(String::from_utf8(first).unwrap().contains("we_android"));
}

#[test]
fn builder_defensively_rejects_web_and_application() {
    for (kind, entry) in [
        (WallpaperKind::Web, "index.html"),
        (WallpaperKind::Application, "demo.exe"),
    ] {
        let source = blocked_source(kind, entry);
        let error = build_export_plan(
            &source,
            ExportRequest::scene(
                PathBuf::from("blocked.mpkg"),
                SceneProfile::Balanced,
                ContentClass::Normal,
            ),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnsupportedWallpaperType);
    }
}

#[test]
fn video_passthrough_must_be_explicitly_proven() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("clip.mp4");
    fs::write(&path, b"fixture").unwrap();
    let source = inspect_source(&path).unwrap();

    let unknown = build_export_plan(
        &source,
        ExportRequest::video(
            PathBuf::from("encoded.mpkg"),
            VideoInputCompatibility::Unknown,
        ),
    )
    .unwrap();
    assert_eq!(unknown.mode, ExportMode::Video { passthrough: false });
    assert_eq!(
        unknown.transformations[0],
        Transformation::SanitizeProperties
    );
    assert_eq!(unknown.helpers, vec![HelperRequirement::H264Encode]);

    let compatible = build_export_plan(
        &source,
        ExportRequest::video(
            PathBuf::from("copied.mpkg"),
            VideoInputCompatibility::AndroidH264Mp4,
        ),
    )
    .unwrap();
    assert_eq!(compatible.mode, ExportMode::Video { passthrough: true });
    assert!(compatible.helpers.is_empty());
}

#[test]
fn request_kind_must_match_the_inspected_source() {
    let (_dir, source) = synthetic_scene_source();
    let error = build_export_plan(
        &source,
        ExportRequest::video(
            PathBuf::from("wrong.mpkg"),
            VideoInputCompatibility::Unknown,
        ),
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArguments);
}

#[test]
fn output_path_must_name_a_destination() {
    let (_dir, source) = synthetic_scene_source();
    let error = build_export_plan(
        &source,
        ExportRequest::scene(PathBuf::new(), SceneProfile::Balanced, ContentClass::Normal),
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArguments);
}

#[test]
fn backend_capabilities_report_missing_helpers_in_request_order() {
    let capabilities = BackendCapabilities {
        protocol_version: 1,
        requirements: vec![HelperRequirement::H264Encode],
    };
    assert!(capabilities.satisfies(&HelperRequirement::H264Encode));
    assert!(!capabilities.satisfies(&HelperRequirement::SceneCapture));
    assert_eq!(
        capabilities.missing_requirements(&[
            HelperRequirement::SceneCapture,
            HelperRequirement::H264Encode,
            HelperRequirement::ResourceTranscode,
        ]),
        vec![
            HelperRequirement::SceneCapture,
            HelperRequirement::ResourceTranscode,
        ]
    );
}
