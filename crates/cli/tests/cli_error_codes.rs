use assert_cmd::cargo::cargo_bin_cmd;
use pkg2mpkg_fixtures::{synthetic_scene_project, synthetic_web_project};

#[test]
fn web_returns_exit_code_three() {
    let project = synthetic_web_project();
    cargo_bin_cmd!("pkg2mpkg")
        .arg("inspect")
        .arg(project.path())
        .assert()
        .code(3)
        .stderr(predicates::str::contains("unsupported wallpaper type"));
}

#[test]
fn json_errors_have_stable_code_stage_and_message() {
    let project = synthetic_web_project();
    let output = cargo_bin_cmd!("pkg2mpkg")
        .arg("inspect")
        .arg(project.path())
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["code"], "unsupported_wallpaper_type");
    assert_eq!(value["stage"], "inspect");
    assert!(value["message"].as_str().unwrap().contains("web"));
}

#[test]
fn scene_export_requires_an_explicit_profile() {
    let project = synthetic_scene_project();
    let output = project.path().join("out.mpkg");
    cargo_bin_cmd!("pkg2mpkg")
        .arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(output)
        .arg("--dry-run")
        .assert()
        .code(2)
        .stderr(predicates::str::contains("--profile"));
}
