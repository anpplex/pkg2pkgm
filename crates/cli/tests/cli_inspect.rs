use std::fs;

use assert_cmd::cargo::cargo_bin_cmd;
use pkg2mpkg_fixtures::{raw_mpkg, synthetic_scene_project};
use tempfile::tempdir;

#[test]
fn inspect_scene_emits_machine_readable_json() {
    let project = synthetic_scene_project();
    let output = cargo_bin_cmd!("pkg2mpkg")
        .args(["inspect"])
        .arg(project.path())
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["kind"], "scene");
    assert_eq!(value["title"], "Synthetic Scene");
    assert_eq!(value["manifest"]["file"], "scene.json");
}

#[test]
fn inspect_human_output_names_the_type_and_entry() {
    let project = synthetic_scene_project();
    cargo_bin_cmd!("pkg2mpkg")
        .args(["inspect"])
        .arg(project.path())
        .assert()
        .success()
        .stdout(predicates::str::contains("type: scene"))
        .stdout(predicates::str::contains("scene.json"));
}

#[test]
fn inspect_mpkg_video_package_reports_kind_and_title() {
    let dir = tempdir().unwrap();
    let package = dir.path().join("video.mpkg");
    fs::write(
        &package,
        raw_mpkg(
            "PKGM0014",
            &[
                (
                    "project.json",
                    br#"{"title":"Shadow Fiend","type":"video","file":"clip.mp4"}"#,
                ),
                ("clip.mp4", b"fake-mp4"),
            ],
        ),
    )
    .unwrap();

    let output = cargo_bin_cmd!("pkg2mpkg")
        .args(["inspect"])
        .arg(&package)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["kind"], "video");
    assert_eq!(value["title"], "Shadow Fiend");
    assert_eq!(value["manifest"]["file"], "clip.mp4");
    assert_eq!(
        value["entry_file"].as_str().unwrap(),
        package.to_str().unwrap()
    );
}
