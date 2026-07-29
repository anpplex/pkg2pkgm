use std::{fs, fs::File};

use pkg2mpkg_core::{ContainerVersion, DesktopPackageArchive, ErrorCode, MpkgArchive};
use pkg2mpkg_fixtures::{raw_mpkg, raw_pkg};
use tempfile::tempdir;

#[test]
fn reads_known_and_other_pkgm_directories_and_payloads() {
    for (magic, version) in [
        ("PKGM0014", ContainerVersion::Pkgm0014),
        ("PKGM0018", ContainerVersion::Pkgm0018),
        ("PKGM0020", ContainerVersion::Pkgm0020),
        ("PKGM9999", ContainerVersion::OtherPkgm { digits: *b"9999" }),
    ] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sample.mpkg");
        fs::write(
            &path,
            raw_mpkg(
                magic,
                &[
                    ("project.json", br#"{"type":"scene"}"#),
                    ("scene.json", br#"{"objects":[]}"#),
                ],
            ),
        )
        .unwrap();
        let archive = MpkgArchive::open(&path).unwrap();
        assert_eq!(archive.version(), version);
        assert_eq!(archive.version().as_magic().as_ref(), magic);
        assert_eq!(archive.entries().len(), 2);
        assert_eq!(
            archive.read_entry("project.json").unwrap(),
            br#"{"type":"scene"}"#
        );
    }
}

#[test]
fn opens_video_pkgm0014_and_reads_project_json() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("video.mpkg");
    let project = br#"{"title":"Shadow Fiend","type":"video","file":"clip.mp4"}"#;
    fs::write(
        &path,
        raw_mpkg(
            "PKGM0014",
            &[
                ("preview.jpg", b"jpeg"),
                ("project.json", project),
                ("clip.mp4", b"fake-mp4"),
            ],
        ),
    )
    .unwrap();

    let archive = MpkgArchive::open(&path).unwrap();
    assert_eq!(archive.version(), ContainerVersion::Pkgm0014);
    assert_eq!(archive.entries().len(), 3);
    assert_eq!(archive.read_entry("project.json").unwrap(), project);
    assert_eq!(archive.read_entry("clip.mp4").unwrap(), b"fake-mp4");
}

#[test]
fn preserves_directory_order_and_reports_missing_entries() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("order.mpkg");
    fs::write(&path, raw_mpkg("PKGM0020", &[("z", b"1"), ("a", b"2")])).unwrap();
    let archive = MpkgArchive::open(&path).unwrap();
    assert_eq!(archive.entries()[0].path, "z");
    assert_eq!(archive.entries()[1].path, "a");
    assert_eq!(
        archive.read_entry("missing").unwrap_err().code(),
        ErrorCode::InvalidMpkg
    );
}

#[test]
fn read_entry_detects_truncation_after_open() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("truncated-after-open.mpkg");
    fs::write(&path, raw_mpkg("PKGM0020", &[("a", b"payload")])).unwrap();
    let archive = MpkgArchive::open(&path).unwrap();
    File::create(&path).unwrap();

    assert_eq!(
        archive.read_entry("a").unwrap_err().code(),
        ErrorCode::InvalidMpkg
    );
}

#[test]
fn desktop_reader_opens_pkgv_and_reads_payloads() {
    for magic in ["PKGV0001", "PKGV0005"] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("scene.pkg");
        fs::write(
            &path,
            raw_pkg(
                magic,
                &[
                    ("scene.json", br#"{"objects":[]}"#),
                    ("materials/main.json", br#"{"passes":[]}"#),
                ],
            ),
        )
        .unwrap();
        let archive = DesktopPackageArchive::open(&path).unwrap();
        assert_eq!(archive.magic(), magic);
        assert_eq!(archive.entries().len(), 2);
        assert_eq!(
            archive.read_entry("scene.json").unwrap(),
            br#"{"objects":[]}"#
        );
        assert_eq!(
            archive.read_entry("materials/main.json").unwrap(),
            br#"{"passes":[]}"#
        );
    }
}

#[test]
fn desktop_reader_rejects_pkgm_and_unknown_magic() {
    let dir = tempdir().unwrap();

    let pkgm = dir.path().join("as-desktop.mpkg");
    fs::write(&pkgm, raw_mpkg("PKGM0020", &[("scene.json", br#"{}"#)])).unwrap();
    let error = DesktopPackageArchive::open(&pkgm).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProject);
    assert!(
        error.to_string().contains("PKGV"),
        "expected PKGV magic guidance, got: {error}"
    );

    let random = dir.path().join("random.pkg");
    fs::write(&random, raw_pkg("NOTAPACK", &[("scene.json", br#"{}"#)])).unwrap();
    let error = DesktopPackageArchive::open(&random).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProject);

    // MPKG still rejects PKGV with InvalidMpkg (mobile-only open).
    let pkgv = dir.path().join("desktop.pkg");
    fs::write(&pkgv, raw_pkg("PKGV0001", &[("scene.json", br#"{}"#)])).unwrap();
    let error = MpkgArchive::open(&pkgv).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidMpkg);
}

#[test]
fn desktop_reader_rejects_path_escapes_as_invalid_project() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("escape.pkg");
    // Craft via raw_pkg with a hostile path component — raw_pkg writes bytes as-is.
    fs::write(&path, raw_pkg("PKGV0001", &[("../escape.txt", b"x")])).unwrap();
    let error = DesktopPackageArchive::open(&path).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProject);
    assert!(
        error.to_string().to_ascii_lowercase().contains("path")
            || error.to_string().to_ascii_lowercase().contains("unsafe"),
        "{error}"
    );
}
