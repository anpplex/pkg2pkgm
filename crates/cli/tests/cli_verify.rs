use std::{fs, path::Path};

use assert_cmd::cargo::cargo_bin_cmd;
use pkg2mpkg_fixtures::raw_mpkg;
use tempfile::tempdir;

fn scene_mpkg(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("scene.mpkg");
    fs::write(
        &path,
        raw_mpkg(
            "PKGM0020",
            &[
                (
                    "project.json",
                    br#"{"title":"Packed","type":"scene","file":"scene.json"}"#,
                ),
                ("scene.json", br#"{"objects":[]}"#),
            ],
        ),
    )
    .unwrap();
    path
}

fn web_mpkg(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("web.mpkg");
    fs::write(
        &path,
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
    path
}

#[test]
fn verify_reports_container_and_project_metadata() {
    let dir = tempdir().unwrap();
    let package = scene_mpkg(dir.path());
    let output = cargo_bin_cmd!("pkg2mpkg")
        .arg("verify")
        .arg(&package)
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["version"], "PKGM0020");
    assert_eq!(value["entry_count"], 2);
    assert_eq!(value["project_type"], "scene");
    assert_eq!(value["entries"][0], "project.json");
}

#[test]
fn verify_rejects_web_packages() {
    let dir = tempdir().unwrap();
    let package = web_mpkg(dir.path());
    cargo_bin_cmd!("pkg2mpkg")
        .arg("verify")
        .arg(package)
        .assert()
        .code(3)
        .stderr(predicates::str::contains("unsupported wallpaper type: web"));
}

#[test]
fn verify_rejects_non_mpkg_input_with_exit_four() {
    let dir = tempdir().unwrap();
    let invalid = dir.path().join("invalid.mpkg");
    std::fs::write(&invalid, b"not an archive").unwrap();
    cargo_bin_cmd!("pkg2mpkg")
        .arg("verify")
        .arg(invalid)
        .assert()
        .code(4);
}

#[test]
fn verify_accepts_pkgm0014_video_packages() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("video.mpkg");
    fs::write(
        &path,
        raw_mpkg(
            "PKGM0014",
            &[
                ("preview.jpg", b"jpeg"),
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
        .arg("verify")
        .arg(&path)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["version"], "PKGM0014");
    assert_eq!(value["entry_count"], 3);
    assert_eq!(value["project_type"], "video");
}
