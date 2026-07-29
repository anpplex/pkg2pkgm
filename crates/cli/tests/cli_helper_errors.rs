//! CLI helper / runtime error mapping (Task 6).

mod common;

use std::fs;

use pkg2mpkg_fixtures::{
    dynamic_scene_project, synthetic_application_project, synthetic_web_project,
};
use tempfile::tempdir;

use common::{FakeRuntime, pkg2mpkg};
#[cfg(unix)]
use common::{install_helper, sidecar};

#[test]
fn missing_runtime_exits_five_without_output() {
    let project = dynamic_scene_project();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");

    let mut cmd = pkg2mpkg();
    cmd.arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "high"]);

    #[cfg(not(windows))]
    {
        // Provide wine+runtime flags partially: missing we-runtime → need wine w/o runtime = 2,
        // so exercise pure missing runtime without wine flags → 5 (backend unavailable).
    }

    cmd.assert()
        .code(5)
        .stderr(predicates::str::contains("backend unavailable"));
    assert!(!output.exists());
}

#[test]
fn runtime_is_file_exits_five() {
    let project = dynamic_scene_project();
    let dir = tempdir().unwrap();
    let runtime_file = dir.path().join("not-a-dir");
    fs::write(&runtime_file, b"file").unwrap();
    let output = dir.path().join("out.mpkg");

    let mut cmd = pkg2mpkg();
    cmd.arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "high"])
        .arg("--we-runtime")
        .arg(&runtime_file);

    #[cfg(not(windows))]
    {
        let wine = install_helper(dir.path(), "wine", "success");
        let winepath = install_helper(dir.path(), "winepath", "winepath");
        cmd.arg("--wine")
            .arg(&wine)
            .arg("--winepath")
            .arg(&winepath);
    }

    cmd.assert().code(5);
    assert!(!output.exists());
}

#[test]
fn wrong_layout_missing_compiler_exits_five() {
    let project = dynamic_scene_project();
    let dir = tempdir().unwrap();
    let runtime = dir.path().join("we-runtime");
    fs::create_dir_all(runtime.join("distribution")).unwrap();
    let output = dir.path().join("out.mpkg");

    let mut cmd = pkg2mpkg();
    cmd.arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "high"])
        .arg("--we-runtime")
        .arg(&runtime);

    #[cfg(not(windows))]
    {
        let wine = install_helper(dir.path(), "wine", "success");
        let winepath = install_helper(dir.path(), "winepath", "winepath");
        cmd.arg("--wine")
            .arg(&wine)
            .arg("--winepath")
            .arg(&winepath);
    }

    cmd.assert()
        .code(5)
        .stderr(predicates::str::contains("resource compiler"));
    assert!(!output.exists());
}

#[cfg(unix)]
#[test]
fn compiler_symlink_escape_exits_five() {
    let project = dynamic_scene_project();
    let dir = tempdir().unwrap();
    let runtime = dir.path().join("we-runtime");
    let bin = runtime.join("distribution/bin");
    fs::create_dir_all(&bin).unwrap();
    let outside = dir.path().join("outside-compiler.exe");
    fs::write(&outside, b"escaped").unwrap();
    std::os::unix::fs::symlink(&outside, bin.join("resourcecompiler64.exe")).unwrap();

    let wine = install_helper(dir.path(), "wine", "success");
    let winepath = install_helper(dir.path(), "winepath", "winepath");
    let output = dir.path().join("out.mpkg");

    pkg2mpkg()
        .arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "high"])
        .arg("--we-runtime")
        .arg(&runtime)
        .arg("--wine")
        .arg(&wine)
        .arg("--winepath")
        .arg(&winepath)
        .assert()
        .code(5)
        .stderr(predicates::str::contains("escape"));
    assert!(!output.exists());
    assert!(!sidecar(&wine, ".argv").exists());
}

#[cfg(unix)]
#[test]
fn wine_missing_exits_five() {
    let project = dynamic_scene_project();
    let runtime = FakeRuntime::with_success_compiler();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");

    pkg2mpkg()
        .arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "high"])
        .arg("--we-runtime")
        .arg(&runtime.runtime)
        .arg("--wine")
        .arg(out_dir.path().join("missing-wine"))
        .assert()
        .code(5);
    assert!(!output.exists());
    assert!(!runtime.any_helper_started());
}

#[cfg(unix)]
#[test]
fn wine_non_executable_exits_five() {
    let project = dynamic_scene_project();
    let runtime = FakeRuntime::with_success_compiler();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");
    let wine = out_dir.path().join("wine");
    fs::write(&wine, b"#!/bin/sh\n").unwrap();
    // intentionally leave without +x

    pkg2mpkg()
        .arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "high"])
        .arg("--we-runtime")
        .arg(&runtime.runtime)
        .arg("--wine")
        .arg(&wine)
        .assert()
        .code(5)
        .stderr(predicates::str::contains("executable"));
    assert!(!output.exists());
}

#[test]
fn helper_conversion_failure_exits_six_without_partial_output() {
    let project = dynamic_scene_project();
    let runtime = FakeRuntime::with_compiler_behavior("nonzero");
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");

    let mut cmd = pkg2mpkg();
    cmd.arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "high"]);
    runtime.apply_launch_flags(&mut cmd);

    cmd.assert()
        .code(6)
        .stderr(predicates::str::contains("conversion failed"));
    assert!(!output.exists());
}

#[test]
fn helper_bad_magic_exits_six_without_partial_output() {
    let project = dynamic_scene_project();
    let runtime = FakeRuntime::with_compiler_behavior("bad-magic");
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");

    let mut cmd = pkg2mpkg();
    cmd.arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "high"]);
    runtime.apply_launch_flags(&mut cmd);

    cmd.assert().code(6);
    assert!(!output.exists());
}

#[cfg(unix)]
#[test]
fn winepath_runtime_failure_exits_six_without_partial_output() {
    let project = dynamic_scene_project();
    let dir = tempdir().unwrap();
    let runtime = dir.path().join("we-runtime");
    let bin = runtime.join("distribution/bin");
    fs::create_dir_all(&bin).unwrap();
    let compiler = bin.join("resourcecompiler64.exe");
    fs::copy(
        env!("CARGO_BIN_EXE_pkg2mpkg-cli-fake-resource-compiler"),
        &compiler,
    )
    .unwrap();
    fs::write(sidecar(&compiler, ".control"), "success\n").unwrap();

    let wine = install_helper(dir.path(), "wine", "success");
    // winepath that starts and exits nonzero
    let winepath = install_helper(dir.path(), "winepath", "nonzero");
    let output = dir.path().join("out.mpkg");

    pkg2mpkg()
        .arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "high"])
        .arg("--we-runtime")
        .arg(&runtime)
        .arg("--wine")
        .arg(&wine)
        .arg("--winepath")
        .arg(&winepath)
        .assert()
        .code(6);
    assert!(!output.exists());
}

#[test]
fn web_export_exits_three_before_runtime_validation() {
    let project = synthetic_web_project();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");
    let bogus_runtime = out_dir.path().join("missing-runtime");

    pkg2mpkg()
        .arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "high"])
        .arg("--we-runtime")
        .arg(&bogus_runtime)
        .assert()
        .code(3)
        .stderr(predicates::str::contains("unsupported wallpaper type"));
    assert!(!output.exists());
}

#[test]
fn application_export_exits_three_before_runtime_validation() {
    let project = synthetic_application_project();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("out.mpkg");
    let bogus_runtime = out_dir.path().join("missing-runtime");

    pkg2mpkg()
        .arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "high"])
        .arg("--we-runtime")
        .arg(&bogus_runtime)
        .assert()
        .code(3)
        .stderr(predicates::str::contains("unsupported wallpaper type"));
    assert!(!output.exists());
}
