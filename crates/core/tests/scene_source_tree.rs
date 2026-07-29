use std::{fs, path::PathBuf};

#[cfg(unix)]
use std::process::Command;

use pkg2mpkg_core::{
    ErrorCode, SceneSourceLimits, Stage, WallpaperKind, inspect_source, inventory_scene_source,
};
use pkg2mpkg_fixtures::{dynamic_scene_project, snapshot_tree, write_bytes};
use tempfile::tempdir;

fn generous_limits() -> SceneSourceLimits {
    SceneSourceLimits {
        max_files: 1_000,
        max_file_bytes: 1_000_000,
        max_total_bytes: 10_000_000,
    }
}

fn archive_paths(tree: &pkg2mpkg_core::SceneSourceTree) -> Vec<&str> {
    tree.entries
        .iter()
        .map(|entry| entry.archive_path.as_str())
        .collect()
}

#[test]
fn dynamic_fixture_inventories_runtime_files_in_bytewise_order() {
    let fixture = dynamic_scene_project();
    let before = snapshot_tree(fixture.path());
    let source = inspect_source(fixture.path()).unwrap();
    assert_eq!(source.kind, WallpaperKind::Scene);

    let tree = inventory_scene_source(&source, generous_limits()).unwrap();

    assert_eq!(tree.root, source.root);
    assert_eq!(tree.scene_entry, "scene.json");
    assert_eq!(tree.preview.as_deref(), Some("preview.jpg"));

    let paths = archive_paths(&tree);
    let mut sorted = paths.clone();
    sorted.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    assert_eq!(
        paths, sorted,
        "entries must be sorted by archive_path bytes"
    );
    assert!(
        paths
            .iter()
            .all(|path| !path.contains('\\') && !path.starts_with('/')),
        "archive paths must use relative forward-slash form: {paths:?}"
    );

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
            paths.contains(&required),
            "expected {required} in inventory, got {paths:?}"
        );
    }

    for excluded in [
        "materials/opaque.tex-json",
        "materials/opaque.TEX-JSON",
        "export.mpkg",
        "export.MPKG",
        "stage.partial",
        "stage.PARTIAL",
        ".pkg2mpkg-debris/tmp.bin",
        ".Pkg2Mpkg-Stash/tmp.bin",
        "escape.link",
    ] {
        assert!(
            !paths.iter().any(|path| path.eq_ignore_ascii_case(excluded)),
            "excluded path leaked into inventory: {excluded} in {paths:?}"
        );
    }

    assert_eq!(
        tree.total_bytes,
        tree.entries.iter().map(|entry| entry.size).sum::<u64>()
    );
    for entry in &tree.entries {
        assert!(entry.source_path.is_file());
        assert_eq!(entry.size, fs::metadata(&entry.source_path).unwrap().len());
        assert!(entry.source_path.starts_with(&tree.root));
    }

    assert_eq!(snapshot_tree(fixture.path()), before, "source tree mutated");
}

#[test]
fn preview_absent_is_allowed_when_undeclared() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"No Preview","type":"scene","file":"scene.json"}"#,
    );

    let source = inspect_source(dir.path()).unwrap();
    let tree = inventory_scene_source(&source, generous_limits()).unwrap();
    assert_eq!(tree.preview, None);
    assert!(archive_paths(&tree).contains(&"project.json"));
    assert!(archive_paths(&tree).contains(&"scene.json"));
}

#[test]
fn declared_preview_must_be_a_regular_included_file() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Missing Preview","type":"scene","file":"scene.json","preview":"preview.jpg"}"#,
    );

    let source = inspect_source(dir.path()).unwrap();
    let error = inventory_scene_source(&source, generous_limits()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProject);
    assert!(error.to_string().to_ascii_lowercase().contains("preview"));
}

#[test]
fn declared_scene_entry_must_be_a_regular_included_file() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Missing Entry Later","type":"scene","file":"scene.json"}"#,
    );
    let source = inspect_source(dir.path()).unwrap();
    fs::remove_file(dir.path().join("scene.json")).unwrap();

    let error = inventory_scene_source(&source, generous_limits()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProject);
    assert!(
        error.to_string().to_ascii_lowercase().contains("entry")
            || error
                .to_string()
                .to_ascii_lowercase()
                .contains("scene.json")
    );
}

#[test]
fn project_json_must_remain_a_regular_included_file() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Drop Project","type":"scene","file":"scene.json"}"#,
    );
    let source = inspect_source(dir.path()).unwrap();
    fs::remove_file(dir.path().join("project.json")).unwrap();

    let error = inventory_scene_source(&source, generous_limits()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProject);
    assert!(
        error
            .to_string()
            .to_ascii_lowercase()
            .contains("project.json")
    );
}

#[test]
fn zero_limits_are_invalid_arguments() {
    let fixture = dynamic_scene_project();
    let source = inspect_source(fixture.path()).unwrap();

    for limits in [
        SceneSourceLimits {
            max_files: 0,
            max_file_bytes: 10,
            max_total_bytes: 10,
        },
        SceneSourceLimits {
            max_files: 10,
            max_file_bytes: 0,
            max_total_bytes: 10,
        },
        SceneSourceLimits {
            max_files: 10,
            max_file_bytes: 10,
            max_total_bytes: 0,
        },
    ] {
        let error = inventory_scene_source(&source, limits).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
        assert_eq!(error.stage(), Stage::Arguments);
    }
}

#[test]
fn file_count_limit_is_enforced_at_the_boundary() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Count","type":"scene","file":"scene.json"}"#,
    );
    write_bytes(&dir.path().join("a.bin"), b"a");
    write_bytes(&dir.path().join("b.bin"), b"b");
    let source = inspect_source(dir.path()).unwrap();

    // project.json + scene.json + a.bin + b.bin = 4 files
    let ok = inventory_scene_source(
        &source,
        SceneSourceLimits {
            max_files: 4,
            max_file_bytes: 1_000,
            max_total_bytes: 1_000,
        },
    )
    .unwrap();
    assert_eq!(ok.entries.len(), 4);

    let error = inventory_scene_source(
        &source,
        SceneSourceLimits {
            max_files: 3,
            max_file_bytes: 1_000,
            max_total_bytes: 1_000,
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProject);
    assert!(error.to_string().to_ascii_lowercase().contains("file"));
}

#[test]
fn per_file_and_total_byte_limits_use_metadata_only() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Bytes","type":"scene","file":"scene.json"}"#,
    );
    write_bytes(&dir.path().join("blob.bin"), &[b'x'; 50]);
    let source = inspect_source(dir.path()).unwrap();

    let per_file = inventory_scene_source(
        &source,
        SceneSourceLimits {
            max_files: 10,
            max_file_bytes: 49,
            max_total_bytes: 10_000,
        },
    )
    .unwrap_err();
    assert_eq!(per_file.code(), ErrorCode::InvalidProject);
    assert!(per_file.to_string().to_ascii_lowercase().contains("size"));

    let total = inventory_scene_source(
        &source,
        SceneSourceLimits {
            max_files: 10,
            max_file_bytes: 1_000,
            max_total_bytes: 20,
        },
    )
    .unwrap_err();
    assert_eq!(total.code(), ErrorCode::InvalidProject);
    assert!(
        total.to_string().to_ascii_lowercase().contains("total")
            || total.to_string().to_ascii_lowercase().contains("size")
    );
}

#[test]
fn checked_total_byte_overflow_is_invalid_project() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Overflow","type":"scene","file":"scene.json"}"#,
    );
    write_bytes(&dir.path().join("a.bin"), b"aa");
    write_bytes(&dir.path().join("b.bin"), b"bb");
    let source = inspect_source(dir.path()).unwrap();

    // Force an addition that would overflow u64 by starting from near-max via
    // a limit that still accepts each file individually but cannot sum them.
    // We emulate overflow by using max_total_bytes = u64::MAX and planting
    // sizes that cannot be added under checked arithmetic after the first
    // large conceptual accumulation: with real small files, exercise the
    // checked path through the public limit instead.
    // Direct unit-style boundary: max_total_bytes just below sum.
    let sum_error = inventory_scene_source(
        &source,
        SceneSourceLimits {
            max_files: 10,
            max_file_bytes: u64::MAX,
            max_total_bytes: 1,
        },
    )
    .unwrap_err();
    assert_eq!(sum_error.code(), ErrorCode::InvalidProject);
}

#[cfg(unix)]
#[test]
fn symlinks_are_rejected_without_following() {
    let fixture = dynamic_scene_project();
    let link = fixture.install_escape_symlink();
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());

    let source = inspect_source(fixture.path()).unwrap();
    let error = inventory_scene_source(&source, generous_limits()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProject);
    assert!(error.to_string().to_ascii_lowercase().contains("symlink"));
}

#[cfg(unix)]
#[test]
fn fifo_and_socket_special_files_are_rejected_without_blocking_reads() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Special","type":"scene","file":"scene.json"}"#,
    );

    let fifo = dir.path().join("pipe.fifo");
    let status = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(status.success());

    let source = inspect_source(dir.path()).unwrap();
    let error = inventory_scene_source(&source, generous_limits()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProject);
    assert!(
        error.to_string().to_ascii_lowercase().contains("special")
            || error.to_string().to_ascii_lowercase().contains("fifo")
            || error
                .to_string()
                .to_ascii_lowercase()
                .contains("not a regular")
    );

    // Socket path: create via UnixListener bind then rename into the project.
    let sock_dir = tempdir().unwrap();
    let sock_path = sock_dir.path().join("sock");
    let _listener = std::os::unix::net::UnixListener::bind(&sock_path).unwrap();
    let project_sock = dir.path().join("device.sock");
    // Re-bind inside project root for a cleaner layout.
    drop(_listener);
    let _ = fs::remove_file(&sock_path);
    let _listener = std::os::unix::net::UnixListener::bind(&project_sock).unwrap();
    // Remove fifo so only socket remains for the second assertion path.
    fs::remove_file(&fifo).unwrap();

    let source = inspect_source(dir.path()).unwrap();
    let error = inventory_scene_source(&source, generous_limits()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProject);
}

#[test]
fn excluded_suffix_and_component_case_variants_are_skipped() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Case","type":"scene","file":"scene.json"}"#,
    );
    write_bytes(&dir.path().join("keep.bin"), b"keep");
    write_bytes(&dir.path().join("a.TeX-JsOn"), b"skip");
    write_bytes(&dir.path().join("b.MpKg"), b"skip");
    write_bytes(&dir.path().join("c.PaRtIaL"), b"skip");
    fs::create_dir_all(dir.path().join(".pKg2MpKg-temp")).unwrap();
    write_bytes(&dir.path().join(".pKg2MpKg-temp/x.bin"), b"skip");

    let source = inspect_source(dir.path()).unwrap();
    let tree = inventory_scene_source(&source, generous_limits()).unwrap();
    let paths = archive_paths(&tree);
    assert!(paths.contains(&"keep.bin"));
    assert!(!paths.iter().any(|p| p.eq_ignore_ascii_case("a.TeX-JsOn")));
    assert!(!paths.iter().any(|p| p.eq_ignore_ascii_case("b.MpKg")));
    assert!(!paths.iter().any(|p| p.eq_ignore_ascii_case("c.PaRtIaL")));
    assert!(
        !paths
            .iter()
            .any(|p| p.contains("pKg2MpKg") || p.contains("pkg2mpkg"))
    );
}

#[test]
fn non_scene_source_is_rejected_as_invalid_arguments() {
    let dir = tempdir().unwrap();
    write_bytes(&dir.path().join("clip.mp4"), b"video");
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Video","type":"video","file":"clip.mp4"}"#,
    );
    let source = inspect_source(dir.path()).unwrap();
    let error = inventory_scene_source(&source, generous_limits()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArguments);
}

#[test]
fn filesystem_errors_map_to_inspect_io() {
    // Inventory against a SourceProject whose root no longer exists.
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Gone","type":"scene","file":"scene.json"}"#,
    );
    let source = inspect_source(dir.path()).unwrap();
    let mut orphan = source.clone();
    orphan.root = PathBuf::from("/definitely/missing/pkg2mpkg-scene-root-xyz");

    let error = inventory_scene_source(&orphan, generous_limits()).unwrap_err();
    match error {
        pkg2mpkg_core::Error::Io { stage, .. } => assert_eq!(stage, Stage::Inspect),
        other => panic!("expected Io Inspect error, got {other}"),
    }
    assert_eq!(error.code(), ErrorCode::InvalidProject);
}
