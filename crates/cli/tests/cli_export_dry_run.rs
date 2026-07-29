use assert_cmd::cargo::cargo_bin_cmd;
use pkg2mpkg_fixtures::{synthetic_scene_project, synthetic_video_project};
use tempfile::tempdir;

#[test]
fn scene_dry_run_emits_plan_without_creating_output() {
    let project = synthetic_scene_project();
    let output = project.path().join("out.mpkg");
    let result = cargo_bin_cmd!("pkg2mpkg")
        .arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "balanced", "--dry-run", "--json"])
        .output()
        .unwrap();
    assert!(result.status.success());
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["mode"]["mode"], "scene_dynamic");
    assert_eq!(value["mode"]["reduction"], "high_quality");
    assert_eq!(value["output"], output.to_string_lossy().as_ref());
    assert!(!output.exists());
}

#[test]
fn non_dry_run_without_runtime_is_backend_unavailable_and_never_creates_output() {
    let project = synthetic_scene_project();
    let output = project.path().join("out.mpkg");
    cargo_bin_cmd!("pkg2mpkg")
        .arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "high"])
        .assert()
        .code(5)
        .stderr(predicates::str::contains("backend unavailable"));
    assert!(!output.exists());
}

#[test]
fn video_dry_run_schedules_h264_without_a_scene_profile() {
    let dir = tempdir().unwrap();
    let project = synthetic_video_project();
    let video = project.entry_path();
    let output = dir.path().join("video.mpkg");
    let result = cargo_bin_cmd!("pkg2mpkg")
        .arg("export")
        .arg(video)
        .arg("--output")
        .arg(&output)
        .args(["--dry-run", "--json"])
        .output()
        .unwrap();
    assert!(result.status.success());
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["mode"]["mode"], "video");
    assert_eq!(value["mode"]["passthrough"], false);
    assert!(!output.exists());
}
