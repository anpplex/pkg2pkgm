use std::{fs, fs::File, io::Write};

use pkg2mpkg_core::{ContainerVersion, ErrorCode, MpkgArchive, MpkgBuilder, OverwritePolicy};
use tempfile::tempdir;

fn partials(dir: &std::path::Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".partial"))
        .collect()
}

#[test]
fn deny_preserves_existing_output() {
    let dir = tempdir().unwrap();
    let output = dir.path().join("out.mpkg");
    fs::write(&output, b"original").unwrap();
    let mut builder = MpkgBuilder::new(ContainerVersion::Pkgm0020);
    builder.add_bytes("project.json", b"{}".to_vec()).unwrap();
    let error = builder
        .write_atomic(&output, OverwritePolicy::Deny)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::OutputIo);
    assert_eq!(fs::read(output).unwrap(), b"original");
    assert!(partials(dir.path()).is_empty());
}

#[test]
fn replace_atomically_overwrites_an_existing_file() {
    let dir = tempdir().unwrap();
    let output = dir.path().join("out.mpkg");
    fs::write(&output, b"original").unwrap();
    let mut builder = MpkgBuilder::new(ContainerVersion::Pkgm0020);
    builder
        .add_bytes("project.json", br#"{"type":"scene"}"#.to_vec())
        .unwrap();
    builder
        .write_atomic(&output, OverwritePolicy::Replace)
        .unwrap();

    assert_eq!(
        MpkgArchive::open(&output)
            .unwrap()
            .read_entry("project.json")
            .unwrap(),
        br#"{"type":"scene"}"#
    );
    assert!(partials(dir.path()).is_empty());
}

#[test]
fn missing_source_leaves_no_output_or_partial() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("scene.json");
    let output = dir.path().join("out.mpkg");
    fs::write(&source, b"{}").unwrap();
    let mut builder = MpkgBuilder::new(ContainerVersion::Pkgm0020);
    builder.add_file("scene.json", &source).unwrap();
    fs::remove_file(source).unwrap();
    assert!(
        builder
            .write_atomic(&output, OverwritePolicy::Deny)
            .is_err()
    );
    assert!(!output.exists());
    assert!(partials(dir.path()).is_empty());
}

#[test]
fn source_size_change_is_rejected_before_packaging() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("scene.json");
    let output = dir.path().join("out.mpkg");
    fs::write(&source, b"{}").unwrap();
    let mut builder = MpkgBuilder::new(ContainerVersion::Pkgm0020);
    builder.add_file("scene.json", &source).unwrap();
    File::options()
        .append(true)
        .open(&source)
        .unwrap()
        .write_all(b"changed")
        .unwrap();

    assert!(
        builder
            .write_atomic(&output, OverwritePolicy::Deny)
            .is_err()
    );
    assert!(!output.exists());
    assert!(partials(dir.path()).is_empty());
}

#[test]
fn duplicate_and_unsafe_paths_are_rejected_before_writing() {
    let mut builder = MpkgBuilder::new(ContainerVersion::Pkgm0020);
    builder.add_bytes("a", vec![1]).unwrap();
    let duplicate = builder.add_bytes("a", vec![2]).unwrap_err();
    assert_eq!(duplicate.code(), ErrorCode::InvalidMpkg);
    let unsafe_path = builder.add_bytes("../escape", vec![3]).unwrap_err();
    assert_eq!(unsafe_path.code(), ErrorCode::InvalidMpkg);
}

#[test]
fn four_gib_sparse_source_is_rejected_before_copy() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("huge.bin");
    File::create(&source)
        .unwrap()
        .set_len(4_u64 * 1024 * 1024 * 1024)
        .unwrap();
    let output = dir.path().join("out.mpkg");
    let mut builder = MpkgBuilder::new(ContainerVersion::Pkgm0020);
    builder.add_file("huge.bin", &source).unwrap();
    let error = builder
        .write_atomic(&output, OverwritePolicy::Deny)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::PackageTooLarge);
    assert!(!output.exists());
    assert!(partials(dir.path()).is_empty());
}
