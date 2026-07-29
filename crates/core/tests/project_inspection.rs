use std::fs;

use pkg2mpkg_core::{ErrorCode, WallpaperKind, inspect_source, source_requires_package_unpack};
use pkg2mpkg_fixtures::{
    raw_mpkg, synthetic_application_project, synthetic_scene_project, synthetic_video_project,
    synthetic_web_project,
};
use tempfile::tempdir;

#[test]
fn scene_project_preserves_unknown_fields() {
    let project = synthetic_scene_project();
    let source = inspect_source(project.path()).unwrap();
    assert_eq!(source.kind, WallpaperKind::Scene);
    assert_eq!(source.title, "Synthetic Scene");
    assert_eq!(source.manifest.raw()["vendor"]["x"], 7);
}

#[test]
fn explicit_web_and_application_fixtures_are_rejected() {
    for project in [synthetic_web_project(), synthetic_application_project()] {
        assert_eq!(
            inspect_source(project.path()).unwrap_err().code(),
            ErrorCode::UnsupportedWallpaperType
        );
    }
}

#[test]
fn html_and_exe_cannot_bypass_the_type_gate_when_type_is_missing() {
    for (entry, expected) in [
        ("index.html", WallpaperKind::Web),
        ("demo.exe", WallpaperKind::Application),
    ] {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(entry), b"x").unwrap();
        fs::write(
            dir.path().join("project.json"),
            format!(r#"{{"title":"blocked","file":"{entry}"}}"#),
        )
        .unwrap();
        let error = inspect_source(dir.path()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnsupportedWallpaperType);
        assert!(error.to_string().contains(expected.as_str()));
    }
}

#[test]
fn explicit_web_type_is_rejected_even_with_a_scene_extension() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("scene.json"), br#"{"camera":{}}"#).unwrap();
    fs::write(
        dir.path().join("project.json"),
        br#"{"title":"blocked","type":"WEB","file":"scene.json"}"#,
    )
    .unwrap();

    let error = inspect_source(dir.path()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::UnsupportedWallpaperType);
}

#[test]
fn missing_type_scene_requires_a_scene_root_marker() {
    let valid = tempdir().unwrap();
    fs::write(valid.path().join("scene.json"), br#"{"objects":[]}"#).unwrap();
    fs::write(
        valid.path().join("project.json"),
        br#"{"title":"inferred","file":"scene.json"}"#,
    )
    .unwrap();
    assert_eq!(
        inspect_source(valid.path()).unwrap().kind,
        WallpaperKind::Scene
    );

    let invalid = tempdir().unwrap();
    fs::write(invalid.path().join("data.json"), br#"{"unrelated":true}"#).unwrap();
    fs::write(
        invalid.path().join("project.json"),
        br#"{"title":"ambiguous","file":"data.json"}"#,
    )
    .unwrap();
    assert_eq!(
        inspect_source(invalid.path()).unwrap_err().code(),
        ErrorCode::InvalidProject
    );
}

#[test]
fn direct_mp4_creates_a_minimal_video_project() {
    let project = synthetic_video_project();
    let video = project.entry_path();

    let source = inspect_source(&video).unwrap();
    assert_eq!(source.kind, WallpaperKind::Video);
    assert_eq!(source.title, "clip");
    assert_eq!(source.entry_file, video);
    assert!(source.project_file.is_none());
    assert_eq!(source.manifest.raw()["type"], "video");
}

#[test]
fn project_entry_cannot_escape_the_project_root() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("project.json"),
        br#"{"title":"escape","type":"video","file":"../secret.mp4"}"#,
    )
    .unwrap();

    let error = inspect_source(dir.path()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProject);
}

#[test]
fn pkg_input_finds_a_project_manifest_one_directory_above() {
    let dir = tempdir().unwrap();
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let package = data.join("scene.pkg");
    fs::write(&package, b"fixture").unwrap();
    fs::write(
        dir.path().join("project.json"),
        br#"{"title":"nested","type":"scene","file":"data/scene.pkg"}"#,
    )
    .unwrap();

    let source = inspect_source(&package).unwrap();
    assert_eq!(source.root, dir.path());
    assert_eq!(source.entry_file, package);
    assert_eq!(source.kind, WallpaperKind::Scene);
    assert!(source_requires_package_unpack(&source));
}

#[test]
fn missing_scene_json_uses_its_exact_sibling_scene_pkg() {
    let dir = tempdir().unwrap();
    let package = dir.path().join("scene.pkg");
    fs::write(&package, b"fixture").unwrap();
    fs::write(
        dir.path().join("project.json"),
        br#"{"title":"workshop","type":"scene","file":"scene.json"}"#,
    )
    .unwrap();

    let source = inspect_source(dir.path()).unwrap();
    assert_eq!(source.entry_file, package);
    assert_eq!(source.manifest.entry(), Some("scene.json"));
    assert!(source_requires_package_unpack(&source));
}

#[test]
fn missing_scene_json_does_not_scan_for_a_different_pkg() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("other.pkg"), b"fixture").unwrap();
    fs::write(
        dir.path().join("project.json"),
        br#"{"title":"ambiguous","type":"scene","file":"scene.json"}"#,
    )
    .unwrap();

    assert_eq!(
        inspect_source(dir.path()).unwrap_err().code(),
        ErrorCode::InvalidProject
    );
}

#[test]
fn existing_scene_json_wins_over_its_sibling_scene_pkg() {
    let dir = tempdir().unwrap();
    let scene = dir.path().join("scene.json");
    fs::write(&scene, br#"{"objects":[]}"#).unwrap();
    fs::write(dir.path().join("scene.pkg"), b"fixture").unwrap();
    fs::write(
        dir.path().join("project.json"),
        br#"{"title":"loose","type":"scene","file":"scene.json"}"#,
    )
    .unwrap();

    let source = inspect_source(dir.path()).unwrap();
    assert_eq!(source.entry_file, scene);
    assert!(!source_requires_package_unpack(&source));
}

#[cfg(unix)]
#[test]
fn dangling_scene_json_symlink_does_not_fall_back_to_sibling_scene_pkg() {
    use std::os::unix::fs::symlink;

    let dir = tempdir().unwrap();
    symlink("missing-scene.json", dir.path().join("scene.json")).unwrap();
    fs::write(dir.path().join("scene.pkg"), b"fixture").unwrap();
    fs::write(
        dir.path().join("project.json"),
        br#"{"title":"dangling","type":"scene","file":"scene.json"}"#,
    )
    .unwrap();

    let error = inspect_source(dir.path()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProject);
    assert!(error.to_string().contains("project entry is not a file"));
}

#[test]
fn mpkg_video_package_inspects_without_loose_entry_files() {
    let dir = tempdir().unwrap();
    let package = dir.path().join("Shadow-fiend.mpkg");
    fs::write(
        &package,
        raw_mpkg(
            "PKGM0014",
            &[
                ("preview.jpg", b"jpeg"),
                (
                    "project.json",
                    br#"{"title":"Shadow Fiend","type":"video","file":"SF2_4.mp4"}"#,
                ),
                ("SF2_4.mp4", b"fake-video-bytes"),
            ],
        ),
    )
    .unwrap();

    let source = inspect_source(&package).unwrap();
    assert_eq!(source.kind, WallpaperKind::Video);
    assert_eq!(source.title, "Shadow Fiend");
    assert_eq!(source.root, dir.path());
    assert_eq!(source.entry_file, package);
    assert!(source.project_file.is_none());
    assert_eq!(source.manifest.raw()["file"], "SF2_4.mp4");
    assert_eq!(source.manifest.raw()["type"], "video");
}

#[test]
fn mpkg_web_package_is_rejected_on_inspect() {
    let dir = tempdir().unwrap();
    let package = dir.path().join("web.mpkg");
    fs::write(
        &package,
        raw_mpkg(
            "PKGM0020",
            &[
                (
                    "project.json",
                    br#"{"title":"Packed Web","type":"web","file":"index.html"}"#,
                ),
                ("index.html", b"<p>fixture</p>"),
            ],
        ),
    )
    .unwrap();

    assert_eq!(
        inspect_source(&package).unwrap_err().code(),
        ErrorCode::UnsupportedWallpaperType
    );
}
