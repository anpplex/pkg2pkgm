use std::fs;

use pkg2mpkg_core::{ContainerVersion, MpkgArchive, MpkgBuilder, OverwritePolicy};
use tempfile::tempdir;

fn partials(dir: &std::path::Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".partial"))
        .collect()
}

#[test]
fn writes_v20_in_insertion_order_and_round_trips() {
    let dir = tempdir().unwrap();
    let first = dir.path().join("first.mpkg");
    let second = dir.path().join("second.mpkg");
    for output in [&first, &second] {
        let mut builder = MpkgBuilder::new(ContainerVersion::Pkgm0020);
        builder
            .add_bytes("scene.json", br#"{"objects":[]}"#.to_vec())
            .unwrap();
        builder
            .add_bytes("project.json", br#"{"type":"scene"}"#.to_vec())
            .unwrap();
        let report = builder.write_atomic(output, OverwritePolicy::Deny).unwrap();
        assert_eq!(report.entries, 2);
        assert_eq!(report.version, ContainerVersion::Pkgm0020);
        assert_eq!(report.bytes, fs::metadata(output).unwrap().len());
    }
    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
    let archive = MpkgArchive::open(&first).unwrap();
    assert_eq!(archive.entries()[0].path, "scene.json");
    assert_eq!(archive.entries()[1].path, "project.json");
    assert_eq!(
        archive.read_entry("scene.json").unwrap(),
        br#"{"objects":[]}"#
    );
    assert!(partials(dir.path()).is_empty());
}

#[test]
fn selected_v18_magic_is_written_and_read_back() {
    let dir = tempdir().unwrap();
    let output = dir.path().join("v18.mpkg");
    let mut builder = MpkgBuilder::new(ContainerVersion::Pkgm0018);
    builder.add_bytes("project.json", b"{}".to_vec()).unwrap();
    builder
        .write_atomic(&output, OverwritePolicy::Deny)
        .unwrap();

    assert_eq!(
        &fs::read(&output).unwrap()[4..12],
        ContainerVersion::Pkgm0018.as_magic().as_bytes()
    );
    assert_eq!(
        MpkgArchive::open(&output).unwrap().version(),
        ContainerVersion::Pkgm0018
    );
}

#[test]
fn one_entry_bytes_match_the_observed_little_endian_directory_layout() {
    let dir = tempdir().unwrap();
    let output = dir.path().join("literal.mpkg");
    let mut builder = MpkgBuilder::new(ContainerVersion::Pkgm0020);
    builder.add_bytes("a", b"x".to_vec()).unwrap();
    builder
        .write_atomic(&output, OverwritePolicy::Deny)
        .unwrap();

    let expected = [
        0x08, 0x00, 0x00, 0x00, b'P', b'K', b'G', b'M', b'0', b'0', b'2', b'0', 0x01, 0x00, 0x00,
        0x00, 0x01, 0x00, 0x00, 0x00, b'a', 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, b'x',
    ];
    assert_eq!(fs::read(output).unwrap(), expected);
}

#[test]
fn writer_rejects_read_only_container_versions() {
    let dir = tempdir().unwrap();
    let output = dir.path().join("readonly.mpkg");
    for version in [
        ContainerVersion::Pkgm0014,
        ContainerVersion::OtherPkgm { digits: *b"9999" },
    ] {
        let mut builder = MpkgBuilder::new(version);
        builder.add_bytes("project.json", b"{}".to_vec()).unwrap();
        let error = builder
            .write_atomic(&output, OverwritePolicy::Deny)
            .unwrap_err();
        assert!(
            error.to_string().contains("PKGM0018") || error.to_string().contains("writer only"),
            "expected writable-version error, got: {error}"
        );
        assert!(!output.exists());
    }
}
