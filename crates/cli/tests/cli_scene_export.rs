//! End-to-end CLI dynamic Scene export (Task 6 happy paths).

mod common;

use std::fs;

use pkg2mpkg_fixtures::dynamic_scene_project;
use tempfile::tempdir;

use common::{FakeRuntime, assert_compiler_flags, pkg2mpkg, read_invocations};

#[test]
fn dry_run_succeeds_without_runtime_and_creates_no_output() {
    let project = dynamic_scene_project();
    let output = project.path().join("out.mpkg");
    let result = pkg2mpkg()
        .arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "high", "--dry-run", "--json"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["mode"]["mode"], "scene_dynamic");
    assert!(!output.exists());
}

#[test]
fn dry_run_with_fake_helpers_never_starts_them() {
    let project = dynamic_scene_project();
    let runtime = FakeRuntime::with_success_compiler();
    let output = project.path().join("out.mpkg");

    let mut cmd = pkg2mpkg();
    cmd.arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "balanced", "--dry-run"]);
    runtime.apply_launch_flags(&mut cmd);

    cmd.assert().success();
    assert!(!output.exists());
    assert!(
        !runtime.any_helper_started(),
        "dry-run must not launch compiler/Wine/winepath"
    );
}

#[test]
fn high_profile_real_export_writes_report_and_hits_helper() {
    let project = dynamic_scene_project();
    let runtime = FakeRuntime::with_success_compiler();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("high.mpkg");

    let mut cmd = pkg2mpkg();
    cmd.arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "high", "--json"]);
    runtime.apply_launch_flags(&mut cmd);

    let result = cmd.output().unwrap();
    assert!(
        result.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.exists());
    assert!(result.stderr.is_empty(), "success must be stdout only");

    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["kind"], "scene");
    assert_eq!(report["mode"]["mode"], "scene_dynamic");
    // dynamic_scene is 640x360 → PixelArt → High maps to high_quality + original
    assert_eq!(report["mode"]["compression"], "high_quality");
    assert_eq!(report["mode"]["reduction"], "high_quality");
    assert_eq!(report["texture_count"], 1);
    assert!(report["output_bytes"].as_u64().unwrap() > 0);
    let text = report.to_string().to_ascii_lowercase();
    for forbidden in [
        "elapsed",
        "duration",
        "timestamp",
        "temp",
        "pid",
        "diagnostic",
    ] {
        assert!(
            !text.contains(forbidden),
            "report must not contain {forbidden}: {text}"
        );
    }

    let argv_path = runtime.compiler_argv_path();
    let invocations = read_invocations(&argv_path);
    assert!(
        !invocations.is_empty(),
        "helper should have been invoked: {argv_path:?}"
    );
    // High + PixelArt → HighQuality (no -c force), shrink 1
    assert_compiler_flags(&invocations[0], false, "1");
}

#[test]
fn balanced_profile_real_export_hits_helper_argv() {
    let project = dynamic_scene_project();
    let runtime = FakeRuntime::with_success_compiler();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("balanced.mpkg");

    let mut cmd = pkg2mpkg();
    cmd.arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "balanced"]);
    runtime.apply_launch_flags(&mut cmd);

    cmd.assert().success();
    assert!(output.exists());

    let invocations = read_invocations(&runtime.compiler_argv_path());
    assert!(!invocations.is_empty());
    // Balanced + PixelArt → HighPerformance + Original
    assert_compiler_flags(&invocations[0], true, "1");
}

#[test]
fn custom_high_quality_original_hits_helper_argv() {
    let project = dynamic_scene_project();
    let runtime = FakeRuntime::with_success_compiler();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("custom-hq.mpkg");

    let mut cmd = pkg2mpkg();
    cmd.arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args([
            "--profile",
            "custom",
            "--compression",
            "high-quality",
            "--reduction",
            "high-quality",
        ]);
    runtime.apply_launch_flags(&mut cmd);

    cmd.assert().success();
    let invocations = read_invocations(&runtime.compiler_argv_path());
    assert!(!invocations.is_empty());
    // HighQuality → no -c force; Original → shrink 1
    assert_compiler_flags(&invocations[0], false, "1");
}

#[test]
fn custom_high_performance_x4_hits_helper_argv() {
    let project = dynamic_scene_project();
    let runtime = FakeRuntime::with_success_compiler();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("custom-x4.mpkg");

    let mut cmd = pkg2mpkg();
    cmd.arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args([
            "--profile",
            "custom",
            "--compression",
            "high-performance",
            "--reduction",
            "reduction-x4",
        ]);
    runtime.apply_launch_flags(&mut cmd);

    cmd.assert().success();
    let invocations = read_invocations(&runtime.compiler_argv_path());
    assert!(!invocations.is_empty());
    assert_compiler_flags(&invocations[0], true, "4");
}

#[test]
fn invalid_custom_combinations_exit_two() {
    let project = dynamic_scene_project();
    let output = project.path().join("out.mpkg");

    pkg2mpkg()
        .arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args([
            "--profile",
            "custom",
            "--compression",
            "high-quality",
            "--dry-run",
        ])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("--reduction"));

    pkg2mpkg()
        .arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args([
            "--profile",
            "high",
            "--compression",
            "high-quality",
            "--dry-run",
        ])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("custom"));
}

#[test]
fn deny_skips_helper_when_output_exists() {
    let project = dynamic_scene_project();
    let runtime = FakeRuntime::with_success_compiler();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("exists.mpkg");
    fs::write(&output, b"pre-existing-bytes").unwrap();

    let mut cmd = pkg2mpkg();
    cmd.arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "high"]);
    runtime.apply_launch_flags(&mut cmd);

    cmd.assert().failure();
    assert_eq!(fs::read(&output).unwrap(), b"pre-existing-bytes");
    assert!(
        !runtime.any_helper_started(),
        "Deny must not launch helper when output exists"
    );
}

#[test]
fn replace_success_is_atomic_and_overwrites() {
    let project = dynamic_scene_project();
    let runtime = FakeRuntime::with_success_compiler();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("replace.mpkg");
    fs::write(&output, b"old-package-bytes").unwrap();

    let mut cmd = pkg2mpkg();
    cmd.arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "high", "--replace"]);
    runtime.apply_launch_flags(&mut cmd);

    cmd.assert().success();
    let bytes = fs::read(&output).unwrap();
    assert_ne!(bytes, b"old-package-bytes");
    // Container header is length-prefixed then PKGM0020 magic.
    assert!(
        bytes.windows(8).any(|window| window == b"PKGM0020"),
        "expected PKGM0020 magic in package, got {bytes:?}"
    );
    assert!(runtime.any_helper_started());
}

#[test]
fn replace_conversion_failure_preserves_existing_bytes() {
    let project = dynamic_scene_project();
    let runtime = FakeRuntime::with_compiler_behavior("nonzero");
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("keep.mpkg");
    let previous = b"preserve-on-failure";
    fs::write(&output, previous).unwrap();

    let mut cmd = pkg2mpkg();
    cmd.arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "high", "--replace"]);
    runtime.apply_launch_flags(&mut cmd);

    let result = cmd.output().unwrap();
    assert_eq!(result.status.code(), Some(6));
    assert_eq!(fs::read(&output).unwrap(), previous);
}

#[test]
fn performance_real_export_is_backend_unavailable_without_helper() {
    let project = dynamic_scene_project();
    let runtime = FakeRuntime::with_success_compiler();
    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("perf.mpkg");

    let mut cmd = pkg2mpkg();
    cmd.arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "performance"]);
    runtime.apply_launch_flags(&mut cmd);

    cmd.assert()
        .code(5)
        .stderr(predicates::str::contains("backend unavailable"));
    assert!(!output.exists());
    // Performance must not launch the resource compiler even with runtime present.
    assert!(
        !runtime.any_helper_started(),
        "performance real export must not launch resource compiler"
    );
}

#[test]
fn performance_dry_run_may_show_plan() {
    let project = dynamic_scene_project();
    let output = project.path().join("out.mpkg");
    let result = pkg2mpkg()
        .arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "performance", "--dry-run", "--json"])
        .output()
        .unwrap();
    assert!(result.status.success());
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["mode"]["mode"], "scene_pre_rendered_video");
    assert!(!output.exists());
}

#[test]
fn winepath_without_wine_is_invalid_arguments() {
    let project = dynamic_scene_project();
    let output = project.path().join("out.mpkg");
    pkg2mpkg()
        .arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args([
            "--profile",
            "high",
            "--winepath",
            "/tmp/winepath",
            "--dry-run",
        ])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("--wine"));
}

#[test]
fn wine_without_we_runtime_is_invalid_arguments() {
    let project = dynamic_scene_project();
    let output = project.path().join("out.mpkg");
    pkg2mpkg()
        .arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args(["--profile", "high", "--wine", "/tmp/wine", "--dry-run"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("--we-runtime"));
}

#[cfg(windows)]
#[test]
fn windows_rejects_wine_flags() {
    let project = dynamic_scene_project();
    let output = project.path().join("out.mpkg");
    pkg2mpkg()
        .arg("export")
        .arg(project.path())
        .arg("--output")
        .arg(&output)
        .args([
            "--profile",
            "high",
            "--we-runtime",
            "C:\\we",
            "--wine",
            "C:\\wine",
            "--dry-run",
        ])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("Wine"));
}
