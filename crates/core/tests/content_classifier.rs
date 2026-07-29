use std::fs;

use pkg2mpkg_core::{ContentClass, classify_content_class, inspect_source};
use pkg2mpkg_fixtures::raw_pkg;
use tempfile::tempdir;

fn scene_with(
    width: u32,
    height: u32,
    tags: &[&str],
) -> (tempfile::TempDir, pkg2mpkg_core::SourceProject) {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("scene.json"),
        format!(
            r#"{{"general":{{"orthogonalprojection":{{"width":{width},"height":{height}}}}},"objects":[]}}"#
        ),
    )
    .unwrap();
    fs::write(
        dir.path().join("project.json"),
        serde_json::to_vec(&serde_json::json!({
            "title": "Classified",
            "type": "scene",
            "file": "scene.json",
            "tags": tags,
        }))
        .unwrap(),
    )
    .unwrap();
    let source = inspect_source(dir.path()).unwrap();
    (dir, source)
}

#[test]
fn scene_below_windows_pixel_threshold_is_pixel_art() {
    let (_dir, source) = scene_with(343, 193, &[]);
    assert_eq!(
        classify_content_class(&source).unwrap(),
        ContentClass::PixelArt
    );
}

#[test]
fn ordinary_hd_scene_is_normal() {
    let (_dir, source) = scene_with(1920, 1080, &[]);
    assert_eq!(
        classify_content_class(&source).unwrap(),
        ContentClass::Normal
    );
}

#[test]
fn resolution_tag_above_full_hd_uses_windows_uhd_priority() {
    let (_dir, source) = scene_with(343, 193, &["3840 x 2160"]);
    assert_eq!(classify_content_class(&source).unwrap(), ContentClass::Uhd);
}

#[test]
fn packaged_scene_uses_the_embedded_projection_for_classification() {
    for manifest_entry in ["scene.json", "scene.pkg"] {
        let dir = tempdir().unwrap();
        let scene =
            br#"{"general":{"orthogonalprojection":{"width":343,"height":193}},"objects":[]}"#;
        fs::write(
            dir.path().join("scene.pkg"),
            raw_pkg("PKGV0005", &[("scene.json", scene)]),
        )
        .unwrap();
        fs::write(
            dir.path().join("project.json"),
            serde_json::to_vec(&serde_json::json!({
                "title": "Packaged pixel art",
                "type": "scene",
                "file": manifest_entry,
            }))
            .unwrap(),
        )
        .unwrap();

        let source = inspect_source(dir.path()).unwrap();

        assert_eq!(
            classify_content_class(&source).unwrap(),
            ContentClass::PixelArt,
            "manifest entry: {manifest_entry}"
        );
    }
}

#[test]
fn packaged_classifier_uses_the_same_strict_fallback_policy_as_unpack() {
    let dir = tempdir().unwrap();
    let scene = br#"{"camera":{},"general":{"orthogonalprojection":{"width":343,"height":193}},"objects":[]}"#;
    fs::write(
        dir.path().join("scene.pkg"),
        raw_pkg(
            "PKGV0005",
            &[
                ("project.json", br#"{"objects":[]}"#),
                ("metadata.json", br#"{"objects":[]}"#),
                ("main.json", scene),
            ],
        ),
    )
    .unwrap();
    fs::write(
        dir.path().join("project.json"),
        br#"{"title":"Fallback","type":"scene","file":"scene.pkg"}"#,
    )
    .unwrap();

    let source = inspect_source(dir.path()).unwrap();

    assert_eq!(
        classify_content_class(&source).unwrap(),
        ContentClass::PixelArt
    );
}

#[test]
fn packaged_classifier_does_not_bypass_an_invalid_canonical_scene() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("scene.pkg"),
        raw_pkg(
            "PKGV0005",
            &[
                ("scene.json", br#"{"not":"a scene"}"#),
                (
                    "main.json",
                    br#"{"camera":{},"general":{"orthogonalprojection":{"width":343,"height":193}},"objects":[]}"#,
                ),
            ],
        ),
    )
    .unwrap();
    fs::write(
        dir.path().join("project.json"),
        br#"{"title":"Invalid canonical","type":"scene","file":"scene.pkg"}"#,
    )
    .unwrap();

    let source = inspect_source(dir.path()).unwrap();

    let error = classify_content_class(&source).unwrap_err();
    assert!(error.to_string().contains("invalid canonical scene.json"));
}

#[test]
fn oversized_loose_and_packaged_scene_documents_fail_before_json_allocation() {
    const MAX_SCENE_DOCUMENT_BYTES: u64 = 64 * 1024 * 1024;

    let loose = tempdir().unwrap();
    let loose_scene = loose.path().join("scene.json");
    fs::File::create(&loose_scene)
        .unwrap()
        .set_len(MAX_SCENE_DOCUMENT_BYTES + 1)
        .unwrap();
    fs::write(
        loose.path().join("project.json"),
        br#"{"title":"Oversized loose","type":"scene","file":"scene.json"}"#,
    )
    .unwrap();
    let loose_source = inspect_source(loose.path()).unwrap();

    let loose_error = classify_content_class(&loose_source).unwrap_err();
    assert!(loose_error.to_string().contains("scene document size"));
    assert!(loose_error.to_string().contains("exceeds"));

    let packaged = tempdir().unwrap();
    let mut package_header = raw_pkg("PKGV0005", &[("scene.json", b"")]);
    let size_offset = package_header.len() - 4;
    let declared_size = u32::try_from(MAX_SCENE_DOCUMENT_BYTES + 1).unwrap();
    package_header[size_offset..].copy_from_slice(&declared_size.to_le_bytes());
    let package_path = packaged.path().join("scene.pkg");
    fs::write(&package_path, &package_header).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&package_path)
        .unwrap()
        .set_len(package_header.len() as u64 + u64::from(declared_size))
        .unwrap();
    fs::write(
        packaged.path().join("project.json"),
        br#"{"title":"Oversized packaged","type":"scene","file":"scene.pkg"}"#,
    )
    .unwrap();
    let packaged_source = inspect_source(packaged.path()).unwrap();

    let packaged_error = classify_content_class(&packaged_source).unwrap_err();
    assert!(packaged_error.to_string().contains("scene document size"));
    assert!(packaged_error.to_string().contains("exceeds"));
}
