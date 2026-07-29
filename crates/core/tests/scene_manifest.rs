use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use pkg2mpkg_core::{
    CompatibilityTarget, Compression, ContainerVersion, ContentClass, ErrorCode, ExportMode,
    ExportPlan, ExportRequest, HelperRequirement, Reduction, SceneProfile, Stage, Transformation,
    WallpaperKind, build_export_plan, build_mobile_scene_project_json, inspect_source,
    validate_scene_references,
};
use pkg2mpkg_fixtures::{dynamic_scene_project, snapshot_tree, write_bytes};
use serde_json::{Value, json};
use tempfile::tempdir;

fn scene_plan_for(source: &pkg2mpkg_core::SourceProject) -> ExportPlan {
    build_export_plan(
        source,
        ExportRequest::scene(
            PathBuf::from("out.mpkg"),
            SceneProfile::High,
            ContentClass::Normal,
        ),
    )
    .unwrap()
}

fn empty_scene_plan() -> ExportPlan {
    ExportPlan {
        source: PathBuf::from("fixture"),
        title: "Fixture".into(),
        kind: WallpaperKind::Scene,
        mode: ExportMode::SceneDynamic {
            compression: Compression::HighPerformance,
            reduction: Reduction::Original,
        },
        compatibility: CompatibilityTarget::WeAndroid { major: 2, minor: 8 },
        properties: Default::default(),
        transformations: vec![
            Transformation::SanitizeProperties,
            Transformation::PackageMpkg {
                version: ContainerVersion::Pkgm0020,
            },
        ],
        helpers: vec![HelperRequirement::ResourceTranscode],
        estimated_size: None,
        output: PathBuf::from("out.mpkg"),
    }
}

fn parse_mobile_json(bytes: &[u8]) -> Value {
    assert!(
        bytes.ends_with(b"\n"),
        "mobile project.json must end with LF"
    );
    assert!(
        !bytes.ends_with(b"\n\n"),
        "mobile project.json must not end with multiple LFs"
    );
    let without_lf = &bytes[..bytes.len() - 1];
    assert!(
        !without_lf.contains(&b'\n'),
        "compact JSON must not contain internal newlines"
    );
    serde_json::from_slice(without_lf).expect("mobile project.json must be valid UTF-8 JSON")
}

fn object_keys_sorted(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            let keys: Vec<&str> = map.keys().map(String::as_str).collect();
            let mut sorted = keys.clone();
            sorted.sort();
            keys == sorted && map.values().all(object_keys_sorted)
        }
        Value::Array(items) => items.iter().all(object_keys_sorted),
        _ => true,
    }
}

fn write_rich_scene_project(root: &Path) {
    write_bytes(
        &root.join("scene.json"),
        br#"{"general":{"orthogonalprojection":{"width":640,"height":360}},"objects":[{"name":"layer"}]}"#,
    );
    write_bytes(
        &root.join("project.json"),
        br#"{
            "title": "Rich Scene",
            "type": "scene",
            "file": "scene.json",
            "preview": "preview.jpg",
            "tags": ["alpha", "beta"],
            "visibility": "public",
            "version": 3,
            "properties": {"top_level": true, "rate": {"value": 9}},
            "general": {
                "properties": {
                    "rate": {"value": 2},
                    "speed": {"value": 1},
                    "schemecolor": {"value": "0 0 0"}
                },
                "supportsaudioprocessing": true
            },
            "vendor": {"fixture": true, "nested": {"keep": 1}},
            "unknown_array": [1, {"z": 1, "a": 2}],
            "workshopid": "999"
        }"#,
    );
    write_bytes(&root.join("preview.jpg"), b"JPEG");
}

#[test]
fn mobile_manifest_preserves_unknown_fields_and_replaces_only_general_properties() {
    let dir = tempdir().unwrap();
    write_rich_scene_project(dir.path());
    let source = inspect_source(dir.path()).unwrap();
    let plan = scene_plan_for(&source);

    // Plan sanitizer drops blacklisted `rate` under /general/properties.
    assert!(!plan.properties.contains_key("rate"));
    assert!(plan.properties.contains_key("speed"));
    assert!(plan.properties.contains_key("schemecolor"));

    let bytes = build_mobile_scene_project_json(&source, &plan).unwrap();
    let value = parse_mobile_json(&bytes);

    assert_eq!(value["title"], "Rich Scene");
    assert_eq!(value["type"], "scene");
    assert_eq!(value["file"], "scene.json");
    assert_eq!(value["preview"], "preview.jpg");
    assert_eq!(value["tags"], json!(["alpha", "beta"]));
    assert_eq!(value["visibility"], "public");
    assert_eq!(value["version"], 3);
    assert_eq!(value["workshopid"], "999");
    assert_eq!(value["vendor"]["fixture"], true);
    assert_eq!(value["vendor"]["nested"]["keep"], 1);
    assert_eq!(value["unknown_array"][0], 1);
    assert_eq!(value["unknown_array"][1]["z"], 1);
    assert_eq!(value["general"]["supportsaudioprocessing"], true);

    // Exact /general/properties comes from the plan override (sanitized).
    let general_props = value["general"]["properties"].as_object().unwrap();
    assert!(!general_props.contains_key("rate"));
    assert_eq!(general_props["speed"]["value"], 1);
    assert_eq!(general_props["schemecolor"]["value"], "0 0 0");

    // Same-named ordinary fields outside the pointer are preserved.
    assert_eq!(value["properties"]["top_level"], true);
    assert_eq!(value["properties"]["rate"]["value"], 9);
}

#[test]
fn mobile_manifest_is_deterministic_key_sorted_compact_utf8_with_one_lf() {
    let dir = tempdir().unwrap();
    write_rich_scene_project(dir.path());
    let source = inspect_source(dir.path()).unwrap();
    let plan = scene_plan_for(&source);

    let first = build_mobile_scene_project_json(&source, &plan).unwrap();
    let second = build_mobile_scene_project_json(&source, &plan).unwrap();
    assert_eq!(first, second);
    assert!(std::str::from_utf8(&first).is_ok());
    assert!(first.ends_with(b"\n"));
    assert_eq!(first.iter().filter(|&&b| b == b'\n').count(), 1);

    let value = parse_mobile_json(&first);
    assert!(object_keys_sorted(&value), "all object keys must be sorted");

    // Compact: no ASCII space after ':' or ',' in the payload body.
    let body = std::str::from_utf8(&first[..first.len() - 1]).unwrap();
    assert!(!body.contains(": "), "compact JSON must not contain ': '");
    assert!(!body.contains(", "), "compact JSON must not contain ', '");
}

#[test]
fn mobile_manifest_inserts_missing_general_and_properties_objects() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Bare","type":"scene","file":"scene.json"}"#,
    );
    let source = inspect_source(dir.path()).unwrap();
    let mut plan = scene_plan_for(&source);
    plan.properties.clear();
    plan.properties.insert("speed".into(), json!({"value": 3}));

    let value = parse_mobile_json(&build_mobile_scene_project_json(&source, &plan).unwrap());
    assert_eq!(value["general"]["properties"]["speed"]["value"], 3);
}

#[test]
fn mobile_manifest_rejects_non_object_general_or_properties() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"BadGeneral","type":"scene","file":"scene.json","general":"nope"}"#,
    );
    let source = inspect_source(dir.path()).unwrap();
    let plan = scene_plan_for(&source);
    let err = build_mobile_scene_project_json(&source, &plan).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidProject);
    assert!(err.to_string().to_ascii_lowercase().contains("general"));

    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"BadProps","type":"scene","file":"scene.json","general":{"properties":[]}}"#,
    );
    let source = inspect_source(dir.path()).unwrap();
    let plan = scene_plan_for(&source);
    let err = build_mobile_scene_project_json(&source, &plan).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidProject);
    assert!(err.to_string().to_ascii_lowercase().contains("properties"));
}

#[test]
fn mobile_manifest_rejects_non_scene_type_and_unsafe_or_missing_entry() {
    // Type change / non-scene kind.
    let video = pkg2mpkg_core::SourceProject {
        root: PathBuf::from("fixture"),
        project_file: Some(PathBuf::from("fixture/project.json")),
        entry_file: PathBuf::from("clip.mp4"),
        title: "Video".into(),
        kind: WallpaperKind::Video,
        manifest: pkg2mpkg_core::ProjectManifest::parse(
            br#"{"title":"Video","type":"video","file":"clip.mp4"}"#,
        )
        .unwrap(),
    };
    let mut plan = empty_scene_plan();
    plan.kind = WallpaperKind::Video;
    plan.mode = ExportMode::Video { passthrough: true };
    plan.helpers.clear();
    let err = build_mobile_scene_project_json(&video, &plan).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidProject);

    // Unsafe entry path in an otherwise Scene-shaped source.
    let unsafe_entry = pkg2mpkg_core::SourceProject {
        root: PathBuf::from("fixture"),
        project_file: Some(PathBuf::from("fixture/project.json")),
        entry_file: PathBuf::from("fixture/../scene.json"),
        title: "Bad".into(),
        kind: WallpaperKind::Scene,
        manifest: pkg2mpkg_core::ProjectManifest::parse(
            br#"{"title":"Bad","type":"scene","file":"../scene.json"}"#,
        )
        .unwrap(),
    };
    let plan = empty_scene_plan();
    let err = build_mobile_scene_project_json(&unsafe_entry, &plan).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidProject);

    // Missing entry file on disk.
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"MissingLater","type":"scene","file":"scene.json"}"#,
    );
    let source = inspect_source(dir.path()).unwrap();
    fs::remove_file(dir.path().join("scene.json")).unwrap();
    let plan = scene_plan_for(&source);
    let err = build_mobile_scene_project_json(&source, &plan).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidProject);
}

#[test]
fn mobile_manifest_does_not_use_top_level_properties_as_mutation_path() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );
    // Only top-level properties; no /general/properties. Plan falls back for
    // sanitization, but mobile mutation must still write /general/properties.
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"TopOnly","type":"scene","file":"scene.json","properties":{"speed":{"value":4},"rate":{"value":2}}}"#,
    );
    let source = inspect_source(dir.path()).unwrap();
    let plan = scene_plan_for(&source);
    assert!(plan.properties.contains_key("speed"));
    assert!(!plan.properties.contains_key("rate"));

    let value = parse_mobile_json(&build_mobile_scene_project_json(&source, &plan).unwrap());
    assert_eq!(value["general"]["properties"]["speed"]["value"], 4);
    assert!(
        !value["general"]["properties"]
            .as_object()
            .unwrap()
            .contains_key("rate")
    );
    // Top-level properties remain the original (unsanitized) copy.
    assert_eq!(value["properties"]["rate"]["value"], 2);
    assert_eq!(value["properties"]["speed"]["value"], 4);
}

#[test]
fn references_discover_local_json_paths_and_register_asset_calls() {
    let fixture = dynamic_scene_project();
    let root = fixture.path();

    // Extend the dynamic fixture with a transitive JSON graph and a script.
    write_bytes(
        &root.join("scene.json"),
        br#"{
            "general": {"orthogonalprojection": {"width": 640, "height": 360}},
            "objects": [
                {"name": "layer", "image": "materials/main.json"},
                {"name": "scripted", "script": "scripts/main.js"},
                {"name": "external_ok", "model": "models/util/fullscreenlayer.json"},
                {"name": "font_global", "file": "fonts/Segment7Standard.otf"}
            ]
        }"#,
    );
    write_bytes(
        &root.join("materials/main.json"),
        br#"{"passes":[{"textures":["materials/opaque.tex"],"shader":"shaders/effects/pulse.frag"}]}"#,
    );
    write_bytes(
        &root.join("scripts/main.js"),
        // Dino-style single quotes, plus double quotes, with noise that must
        // not produce false positives.
        b"// engine.registerAsset('sounds/missing_comment.wav');\n\
          /* engine.registerAsset(\"sounds/missing_block.wav\"); */\n\
          var s = \"engine.registerAsset('sounds/missing_string.wav')\";\n\
          var t = `engine.registerAsset(\"sounds/missing_template.wav\")`;\n\
          var notIt = fakeengine.registerAsset('sounds/missing_ident.wav');\n\
          engine.registerAsset('sounds/click.wav');\n\
          engine.registerAsset(\"nested/deep/note.txt\");\n\
          engine.registerAsset('scripts/extra.js');\n",
    );
    write_bytes(
        &root.join("scripts/extra.js"),
        b"engine.registerAsset(\"unused/extra.bin\");\n",
    );
    let before = snapshot_tree(root);

    let report = validate_scene_references(root, "scene.json").unwrap();
    assert_eq!(report.scene_entry, "scene.json");

    let refs: BTreeMap<&str, ()> = report
        .local_references
        .iter()
        .map(|path| (path.as_str(), ()))
        .collect();
    for required in [
        "scene.json",
        "materials/main.json",
        "materials/opaque.tex",
        "shaders/effects/pulse.frag",
        "scripts/main.js",
        "scripts/extra.js",
        "sounds/click.wav",
        "nested/deep/note.txt",
        "unused/extra.bin",
    ] {
        assert!(
            refs.contains_key(required),
            "expected local reference {required}, got {:?}",
            report.local_references
        );
    }
    for external in [
        "models/util/fullscreenlayer.json",
        "fonts/Segment7Standard.otf",
        "sounds/missing_comment.wav",
        "sounds/missing_block.wav",
        "sounds/missing_string.wav",
        "sounds/missing_template.wav",
        "sounds/missing_ident.wav",
    ] {
        assert!(
            !refs.contains_key(external),
            "external/false-positive leaked into report: {external}"
        );
    }

    // Sorted by archive path bytes.
    let mut sorted = report.local_references.clone();
    sorted.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    assert_eq!(report.local_references, sorted);

    assert_eq!(
        snapshot_tree(root),
        before,
        "reference validation mutated source"
    );
}

#[test]
fn references_reject_traversal_and_unsafe_path_forms() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"objects":[{"file":"../escape.bin"}]}"#,
    );
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Traversal","type":"scene","file":"scene.json"}"#,
    );
    let err = validate_scene_references(dir.path(), "scene.json").unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidProject);
    assert!(
        err.to_string().to_ascii_lowercase().contains("unsafe")
            || err.to_string().contains("..")
            || err.to_string().to_ascii_lowercase().contains("path")
    );

    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"objects":[{"script":"scripts/main.js"}]}"#,
    );
    write_bytes(
        &dir.path().join("scripts/main.js"),
        b"engine.registerAsset('../escape.bin');\n",
    );
    let err = validate_scene_references(dir.path(), "scene.json").unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidProject);
}

#[test]
fn references_fail_when_explicit_register_asset_is_missing() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"objects":[{"script":"scripts/main.js"}]}"#,
    );
    write_bytes(
        &dir.path().join("scripts/main.js"),
        b"engine.registerAsset('sounds/missing.wav');\n",
    );
    let err = validate_scene_references(dir.path(), "scene.json").unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidProject);
    assert!(
        err.to_string().contains("missing")
            || err.to_string().contains("sounds/missing.wav")
            || err.to_string().to_ascii_lowercase().contains("register")
    );
}

#[test]
fn references_do_not_fail_solely_because_we_global_json_paths_are_absent() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{
            "objects": [
                {"model": "models/util/fullscreenlayer.json"},
                {"font": "fonts/Segment7Standard.otf"},
                {"label": "not a path"},
                {"empty": ""}
            ]
        }"#,
    );
    let report = validate_scene_references(dir.path(), "scene.json").unwrap();
    assert_eq!(report.local_references, vec!["scene.json".to_string()]);
}

#[test]
fn references_reject_missing_scene_entry_and_report_stage() {
    let dir = tempdir().unwrap();
    write_bytes(&dir.path().join("other.json"), br#"{"objects":[]}"#);
    let err = validate_scene_references(dir.path(), "scene.json").unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidProject);
    assert_eq!(err.stage(), Stage::Inspect);
}

#[test]
fn build_mobile_scene_uses_dynamic_fixture_without_mutating_source() {
    let fixture = dynamic_scene_project();
    let before = snapshot_tree(fixture.path());
    let source = inspect_source(fixture.path()).unwrap();
    let plan = scene_plan_for(&source);
    let bytes = build_mobile_scene_project_json(&source, &plan).unwrap();
    let value = parse_mobile_json(&bytes);
    assert_eq!(value["type"], "scene");
    assert_eq!(value["file"], "scene.json");
    assert_eq!(value["title"], "Dynamic Fixture");
    assert_eq!(value["vendor"]["fixture"], true);
    assert!(value["general"]["properties"].is_object());
    assert_eq!(snapshot_tree(fixture.path()), before);
}
