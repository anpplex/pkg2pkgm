use std::fs;

use pkg2mpkg_core::{
    ErrorCode, Stage, apply_compat_shaders, load_compat_shader_rule, parse_compat_shader_config,
    workshop_project_id,
};
use pkg2mpkg_fixtures::write_bytes;
use tempfile::tempdir;

fn write_rule(
    zcompat: &std::path::Path,
    project_id: &str,
    maximum_project_id: &str,
    frag: &str,
    vert: &str,
    frag_body: &[u8],
    vert_body: &[u8],
) {
    let rule_dir = zcompat.join(project_id);
    fs::create_dir_all(&rule_dir).unwrap();
    write_bytes(
        &rule_dir.join("config.json"),
        format!(r#"{{"maximumprojectid":"{maximum_project_id}","frag":"{frag}","vert":"{vert}"}}"#)
            .as_bytes(),
    );
    write_bytes(&rule_dir.join(frag), frag_body);
    write_bytes(&rule_dir.join(vert), vert_body);
}

#[test]
fn parses_maximumprojectid_and_shader_basenames() {
    let config = parse_compat_shader_config(
        br#"{"maximumprojectid":"2638335396","frag":"Simple_Audio_Bars.frag","vert":"Simple_Audio_Bars.vert"}"#,
    )
    .unwrap();

    assert_eq!(config.maximum_project_id, 2_638_335_396);
    assert_eq!(config.frag.as_deref(), Some("Simple_Audio_Bars.frag"));
    assert_eq!(config.vert.as_deref(), Some("Simple_Audio_Bars.vert"));
}

#[test]
fn rejects_unsafe_frag_and_vert_paths() {
    // Use serde_json to build the document so path bytes are not re-escaped.
    for unsafe_name in [
        "../evil.frag",
        "/abs.frag",
        "a/b.frag",
        "a\\b.frag",
        "..",
        ".",
        "",
        "evil\0.frag",
        "C:evil.frag",
    ] {
        let value = serde_json::json!({
            "maximumprojectid": "10",
            "frag": unsafe_name,
            "vert": "ok.vert",
        });
        // NUL cannot be represented in JSON strings; exercise the raw parser path.
        let json = if unsafe_name.contains('\0') {
            br#"{"maximumprojectid":"10","frag":"evil\u0000.frag","vert":"ok.vert"}"#.to_vec()
        } else {
            serde_json::to_vec(&value).unwrap()
        };
        let error = parse_compat_shader_config(&json).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidProject, "{unsafe_name:?}");
        assert!(
            error.to_string().to_ascii_lowercase().contains("unsafe")
                || error.to_string().to_ascii_lowercase().contains("path")
                || error.to_string().to_ascii_lowercase().contains("json"),
            "{unsafe_name:?}: {error}"
        );
    }
}

#[test]
fn loads_rule_project_id_from_folder_name() {
    let dir = tempdir().unwrap();
    write_rule(
        dir.path(),
        "2084198056",
        "2638335396",
        "bars.frag",
        "bars.vert",
        b"FRAG",
        b"VERT",
    );

    let rule = load_compat_shader_rule(&dir.path().join("2084198056")).unwrap();
    assert_eq!(rule.project_id, "2084198056");
    assert_eq!(rule.config.maximum_project_id, 2_638_335_396);
    assert!(rule.applies_to("2084198056"));
    assert!(!rule.applies_to("2084198057"));
    assert!(!rule.applies_to("9999999999"));
}

#[test]
fn maximumprojectid_policy_rejects_larger_project_ids() {
    let dir = tempdir().unwrap();
    // Folder named for a large id, but maximum is smaller — must not apply.
    write_rule(
        dir.path(),
        "3000000000",
        "2638335396",
        "x.frag",
        "x.vert",
        b"F",
        b"V",
    );
    let rule = load_compat_shader_rule(&dir.path().join("3000000000")).unwrap();
    assert!(!rule.applies_to("3000000000"));
    assert!(!rule.applies_to("2638335396"));
}

#[test]
fn apply_replaces_only_named_shader_basenames() {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    let zcompat = root.path().join("zcompat");
    fs::create_dir_all(project.join("shaders/effects")).unwrap();
    write_bytes(
        &project.join("shaders/effects/pulse.frag"),
        b"ORIGINAL-FRAG",
    );
    write_bytes(
        &project.join("shaders/effects/pulse.vert"),
        b"ORIGINAL-VERT",
    );
    write_bytes(&project.join("shaders/effects/other.frag"), b"OTHER-FRAG");
    write_bytes(&project.join("keep.bin"), b"KEEP");

    write_rule(
        &zcompat,
        "2078835426",
        "9223372036854775807",
        "pulse.frag",
        "pulse.vert",
        b"COMPAT-FRAG",
        b"COMPAT-VERT",
    );

    let report = apply_compat_shaders(&project, "2078835426", &zcompat).unwrap();
    assert_eq!(report.replaced.len(), 2);
    assert!(
        report
            .replaced
            .iter()
            .any(|path| path.ends_with("pulse.frag"))
    );
    assert!(
        report
            .replaced
            .iter()
            .any(|path| path.ends_with("pulse.vert"))
    );

    assert_eq!(
        fs::read(project.join("shaders/effects/pulse.frag")).unwrap(),
        b"COMPAT-FRAG"
    );
    assert_eq!(
        fs::read(project.join("shaders/effects/pulse.vert")).unwrap(),
        b"COMPAT-VERT"
    );
    assert_eq!(
        fs::read(project.join("shaders/effects/other.frag")).unwrap(),
        b"OTHER-FRAG"
    );
    assert_eq!(fs::read(project.join("keep.bin")).unwrap(), b"KEEP");
}

#[test]
fn no_matching_rule_preserves_original_shader_bytes() {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    let zcompat = root.path().join("zcompat");
    fs::create_dir_all(project.join("shaders")).unwrap();
    write_bytes(&project.join("shaders/pulse.frag"), b"ORIGINAL");
    write_bytes(&project.join("shaders/pulse.vert"), b"ORIGINAL-V");

    // Rule for a different project id.
    write_rule(
        &zcompat,
        "111",
        "999",
        "pulse.frag",
        "pulse.vert",
        b"COMPAT",
        b"COMPAT-V",
    );

    let report = apply_compat_shaders(&project, "222", &zcompat).unwrap();
    assert!(report.replaced.is_empty());
    assert_eq!(
        fs::read(project.join("shaders/pulse.frag")).unwrap(),
        b"ORIGINAL"
    );
    assert_eq!(
        fs::read(project.join("shaders/pulse.vert")).unwrap(),
        b"ORIGINAL-V"
    );
}

#[test]
fn maximumprojectid_miss_preserves_originals_even_when_folder_matches() {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    let zcompat = root.path().join("zcompat");
    fs::create_dir_all(project.join("shaders")).unwrap();
    write_bytes(&project.join("shaders/x.frag"), b"ORIG-F");
    write_bytes(&project.join("shaders/x.vert"), b"ORIG-V");

    write_rule(
        &zcompat,
        "3000000000",
        "100",
        "x.frag",
        "x.vert",
        b"NEW-F",
        b"NEW-V",
    );

    let report = apply_compat_shaders(&project, "3000000000", &zcompat).unwrap();
    assert!(report.replaced.is_empty());
    assert_eq!(fs::read(project.join("shaders/x.frag")).unwrap(), b"ORIG-F");
    assert_eq!(fs::read(project.join("shaders/x.vert")).unwrap(), b"ORIG-V");
}

#[test]
fn missing_zcompat_tree_is_a_no_op() {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    fs::create_dir_all(project.join("shaders")).unwrap();
    write_bytes(&project.join("shaders/a.frag"), b"A");

    let report = apply_compat_shaders(&project, "1", &root.path().join("missing-zcompat")).unwrap();
    assert!(report.replaced.is_empty());
    assert_eq!(fs::read(project.join("shaders/a.frag")).unwrap(), b"A");
}

#[test]
fn invalid_config_json_is_invalid_project() {
    let dir = tempdir().unwrap();
    let rule_dir = dir.path().join("42");
    fs::create_dir_all(&rule_dir).unwrap();
    write_bytes(&rule_dir.join("config.json"), b"not-json");

    let error = load_compat_shader_rule(&rule_dir).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProject);
    assert_eq!(error.stage(), Stage::Inspect);
}

#[test]
fn workshop_project_id_reads_manifest_field() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"W","type":"scene","file":"scene.json","workshopid":"2078835426"}"#,
    );
    write_bytes(
        &dir.path().join("scene.json"),
        br#"{"general":{},"objects":[]}"#,
    );

    let source = pkg2mpkg_core::inspect_source(dir.path()).unwrap();
    assert_eq!(workshop_project_id(&source).as_deref(), Some("2078835426"));
}

#[test]
fn apply_rejects_symlink_replacement_sources() {
    #[cfg(unix)]
    {
        let root = tempdir().unwrap();
        let project = root.path().join("project");
        let zcompat = root.path().join("zcompat");
        fs::create_dir_all(project.join("shaders")).unwrap();
        write_bytes(&project.join("shaders/pulse.frag"), b"ORIG");
        write_bytes(&project.join("shaders/pulse.vert"), b"ORIG-V");

        let rule_dir = zcompat.join("9");
        fs::create_dir_all(&rule_dir).unwrap();
        write_bytes(
            &rule_dir.join("config.json"),
            br#"{"maximumprojectid":"9","frag":"pulse.frag","vert":"pulse.vert"}"#,
        );
        let outside = root.path().join("outside.frag");
        write_bytes(&outside, b"EVIL");
        std::os::unix::fs::symlink(&outside, rule_dir.join("pulse.frag")).unwrap();
        write_bytes(&rule_dir.join("pulse.vert"), b"VERT");

        let error = apply_compat_shaders(&project, "9", &zcompat).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidProject);
        assert!(error.to_string().to_ascii_lowercase().contains("symlink"));
        assert_eq!(
            fs::read(project.join("shaders/pulse.frag")).unwrap(),
            b"ORIG",
            "failed apply must not leave partial replacements that used a symlink source"
        );
    }
}
