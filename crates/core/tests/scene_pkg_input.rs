use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use pkg2mpkg_core::{
    DesktopPackageArchive, ErrorCode, NativeScenePackageUnpackBackend, ScenePackageEntry,
    ScenePackageUnpackBackend, ScenePackageUnpackLimits, ScenePackageUnpackReport,
    ScenePackageUnpackRequest, SceneSourceLimits, Stage, WallpaperKind, inspect_source,
    package_entry_is_raw_scene_pkg, prepare_packaged_scene_source, unpack_scene_package_checked,
    validate_unpacked_scene_tree,
};
use pkg2mpkg_fixtures::{raw_pkg, write_bytes};
use tempfile::tempdir;

fn generous_limits() -> ScenePackageUnpackLimits {
    ScenePackageUnpackLimits {
        max_entries: 1_000,
        max_path_length: 16_384,
        max_file_bytes: 1_000_000,
        max_total_bytes: 10_000_000,
    }
}

fn scene_inventory_limits() -> SceneSourceLimits {
    SceneSourceLimits {
        max_files: 1_000,
        max_file_bytes: 1_000_000,
        max_total_bytes: 10_000_000,
    }
}

/// Hermetic fake: entries are pre-registered relative paths → payload bytes.
struct FakeUnpackBackend {
    packages: Mutex<BTreeMap<PathBuf, BTreeMap<String, Vec<u8>>>>,
}

impl FakeUnpackBackend {
    fn new() -> Self {
        Self {
            packages: Mutex::new(BTreeMap::new()),
        }
    }

    fn register(&self, package: &Path, entries: BTreeMap<String, Vec<u8>>) {
        self.packages
            .lock()
            .unwrap()
            .insert(package.to_path_buf(), entries);
    }
}

impl ScenePackageUnpackBackend for FakeUnpackBackend {
    fn unpack_scene_package(
        &self,
        request: &ScenePackageUnpackRequest,
    ) -> pkg2mpkg_core::Result<ScenePackageUnpackReport> {
        let packages = self.packages.lock().unwrap();
        let entries = packages.get(&request.package).ok_or_else(|| {
            pkg2mpkg_core::Error::BackendUnavailable {
                backend: format!("fake scene package {}", request.package.display()),
            }
        })?;

        fs::create_dir_all(&request.output_dir).map_err(|source| pkg2mpkg_core::Error::Io {
            stage: Stage::Unpack,
            path: request.output_dir.clone(),
            source,
        })?;

        let mut report_entries = Vec::new();
        let mut total_bytes = 0_u64;
        for (path, bytes) in entries {
            if path.len() > request.limits.max_path_length {
                return Err(pkg2mpkg_core::Error::InvalidProject {
                    reason: format!(
                        "entry path length {} exceeds {}",
                        path.len(),
                        request.limits.max_path_length
                    ),
                });
            }
            if report_entries.len() as u32 >= request.limits.max_entries {
                return Err(pkg2mpkg_core::Error::InvalidProject {
                    reason: format!("entry count exceeds limit {}", request.limits.max_entries),
                });
            }
            let size = bytes.len() as u64;
            if size > request.limits.max_file_bytes {
                return Err(pkg2mpkg_core::Error::InvalidProject {
                    reason: format!(
                        "file size {size} exceeds per-file limit {}",
                        request.limits.max_file_bytes
                    ),
                });
            }
            total_bytes = total_bytes.checked_add(size).ok_or_else(|| {
                pkg2mpkg_core::Error::InvalidProject {
                    reason: format!("total byte count overflow while unpacking {path}"),
                }
            })?;
            if total_bytes > request.limits.max_total_bytes {
                return Err(pkg2mpkg_core::Error::InvalidProject {
                    reason: format!(
                        "total size {total_bytes} exceeds total limit {}",
                        request.limits.max_total_bytes
                    ),
                });
            }

            // Reject escape forms before materializing so a hostile archive
            // path cannot write outside output_dir (Path::join allows `..`).
            if !is_safe_archive_path(path) {
                return Err(pkg2mpkg_core::Error::InvalidProject {
                    reason: format!("unsafe unpack archive path: {path:?}"),
                });
            }
            let dest = join_archive_path(&request.output_dir, path);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|source| pkg2mpkg_core::Error::Io {
                    stage: Stage::Unpack,
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            fs::write(&dest, bytes).map_err(|source| pkg2mpkg_core::Error::Io {
                stage: Stage::Unpack,
                path: dest.clone(),
                source,
            })?;
            report_entries.push(ScenePackageEntry {
                path: path.clone(),
                size,
            });
        }

        Ok(ScenePackageUnpackReport {
            output_dir: request.output_dir.clone(),
            entries: report_entries,
            total_bytes,
        })
    }
}

/// Mirror of core `normalize_archive_path`: relative `/` form, no `..` / empty / drive.
fn is_safe_archive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    let drive_prefix = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    !(path.is_empty()
        || path.contains('\0')
        || path.contains('\\')
        || path.starts_with('/')
        || drive_prefix
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == ".."))
}

/// Join root + archive path component-wise (same style as production helpers).
fn join_archive_path(root: &Path, archive_path: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in archive_path.split('/') {
        path.push(part);
    }
    path
}

fn synthetic_scene_pkg_entries() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        (
            "scene.json".into(),
            br#"{"general":{},"objects":[]}"#.to_vec(),
        ),
        ("materials/main.json".into(), br#"{"passes":[]}"#.to_vec()),
        ("shaders/fx.frag".into(), b"FRAG".to_vec()),
    ])
}

fn prepare_native_scene_entry(entries: &[(&str, &[u8])]) -> pkg2mpkg_core::Result<String> {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Native PKGV","type":"scene","file":"scene.pkg"}"#,
    );
    let package = dir.path().join("scene.pkg");
    fs::write(&package, raw_pkg("PKGV0001", entries)).unwrap();
    let source = inspect_source(dir.path()).unwrap();
    let prepared = prepare_packaged_scene_source(
        &source,
        &NativeScenePackageUnpackBackend::new(),
        &dir.path().join("unpacked"),
        generous_limits(),
        scene_inventory_limits(),
    )?;
    Ok(prepared.source.manifest.entry().unwrap().to_owned())
}

#[test]
fn unpack_request_rejects_empty_and_identical_paths() {
    let error =
        ScenePackageUnpackRequest::new(PathBuf::new(), PathBuf::from("out"), generous_limits())
            .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArguments);

    let error = ScenePackageUnpackRequest::new(
        PathBuf::from("scene.pkg"),
        PathBuf::from("scene.pkg"),
        generous_limits(),
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArguments);

    let error = ScenePackageUnpackRequest::new(
        PathBuf::from("scene.pkg"),
        PathBuf::from("out"),
        ScenePackageUnpackLimits {
            max_entries: 0,
            max_path_length: 16_384,
            max_file_bytes: 1,
            max_total_bytes: 1,
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArguments);
}

#[test]
fn fake_backend_unpacks_into_task_owned_directory() {
    let dir = tempdir().unwrap();
    let package = dir.path().join("scene.pkg");
    write_bytes(&package, b"opaque-desktop-pkg-bytes");
    let out = dir.path().join("task-unpack");

    let backend = FakeUnpackBackend::new();
    backend.register(&package, synthetic_scene_pkg_entries());

    let request =
        ScenePackageUnpackRequest::new(package.clone(), out.clone(), generous_limits()).unwrap();
    let report = unpack_scene_package_checked(&backend, &request).unwrap();

    assert_eq!(report.output_dir, out);
    assert_eq!(report.entries.len(), 3);
    assert!(out.join("scene.json").is_file());
    assert!(out.join("materials/main.json").is_file());
    assert!(out.join("shaders/fx.frag").is_file());
    assert!(!out.join("scene.pkg").exists());
}

#[test]
fn checked_unpack_rejects_parent_and_absolute_entry_paths() {
    let dir = tempdir().unwrap();
    let package = dir.path().join("scene.pkg");
    write_bytes(&package, b"pkg");
    let out = dir.path().join("out");

    for bad in [
        "../escape.txt",
        "/abs.txt",
        "a\\b.txt",
        "a//b.txt",
        "./x.txt",
    ] {
        // Backend reports unsafe paths without writing; checked layer rejects.
        let backend = ReportingBackend {
            entries: vec![ScenePackageEntry {
                path: bad.to_string(),
                size: 1,
            }],
            write: false,
        };
        let request =
            ScenePackageUnpackRequest::new(package.clone(), out.clone(), generous_limits())
                .unwrap();
        let error = unpack_scene_package_checked(&backend, &request).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidProject, "{bad}");
        assert!(
            error.to_string().to_ascii_lowercase().contains("path")
                || error.to_string().to_ascii_lowercase().contains("unsafe"),
            "{bad}: {error}"
        );
    }
}

#[test]
fn checked_unpack_rejects_paths_longer_than_mpkg_max() {
    let dir = tempdir().unwrap();
    let package = dir.path().join("scene.pkg");
    write_bytes(&package, b"pkg");
    let out = dir.path().join("out");
    let long_name = "a".repeat(16_385);
    let backend = ReportingBackend {
        entries: vec![ScenePackageEntry {
            path: long_name,
            size: 1,
        }],
        write: false,
    };
    let request = ScenePackageUnpackRequest::new(package, out, generous_limits()).unwrap();
    let error = unpack_scene_package_checked(&backend, &request).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProject);
    assert!(error.to_string().to_ascii_lowercase().contains("path"));
}

#[test]
fn checked_unpack_rejects_entry_count_over_limit() {
    let dir = tempdir().unwrap();
    let package = dir.path().join("scene.pkg");
    write_bytes(&package, b"pkg");
    let out = dir.path().join("out");
    let backend = ReportingBackend {
        entries: vec![
            ScenePackageEntry {
                path: "a.txt".into(),
                size: 1,
            },
            ScenePackageEntry {
                path: "b.txt".into(),
                size: 1,
            },
        ],
        write: false,
    };
    let request = ScenePackageUnpackRequest::new(
        package,
        out,
        ScenePackageUnpackLimits {
            max_entries: 1,
            max_path_length: 16_384,
            max_file_bytes: 100,
            max_total_bytes: 100,
        },
    )
    .unwrap();
    let error = unpack_scene_package_checked(&backend, &request).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProject);
}

#[test]
fn package_entry_detects_raw_scene_pkg() {
    assert!(package_entry_is_raw_scene_pkg("scene.pkg"));
    assert!(package_entry_is_raw_scene_pkg("data/scene.pkg"));
    assert!(package_entry_is_raw_scene_pkg("SCENE.PKG"));
    assert!(!package_entry_is_raw_scene_pkg("scene.json"));
    assert!(!package_entry_is_raw_scene_pkg("materials/main.json"));
}

#[test]
fn checked_unpack_never_succeeds_when_only_raw_scene_pkg_is_present() {
    let dir = tempdir().unwrap();
    let package = dir.path().join("desktop.pkg");
    write_bytes(&package, b"pkg");
    let out = dir.path().join("out");

    // Backend "succeeds" but only materializes the raw package — must fail.
    let backend = FakeUnpackBackend::new();
    backend.register(
        &package,
        BTreeMap::from([("scene.pkg".into(), b"still-raw".to_vec())]),
    );
    let request = ScenePackageUnpackRequest::new(package, out.clone(), generous_limits()).unwrap();
    let error = unpack_scene_package_checked(&backend, &request).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ConversionFailed);
    assert!(
        error.to_string().to_ascii_lowercase().contains("scene.pkg")
            || error.to_string().to_ascii_lowercase().contains("unpack"),
        "{error}"
    );
}

#[test]
fn prepare_packaged_scene_runs_inventory_on_task_owned_tree() {
    let dir = tempdir().unwrap();
    // Workshop-style layout: project.json + packaged entry.
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Packaged","type":"scene","file":"scene.pkg","preview":"preview.jpg"}"#,
    );
    write_bytes(&dir.path().join("preview.jpg"), b"JPEG");
    let package = dir.path().join("scene.pkg");
    write_bytes(&package, b"opaque-desktop-pkg");

    let source = inspect_source(dir.path()).unwrap();
    assert_eq!(source.kind, WallpaperKind::Scene);
    assert!(package_entry_is_raw_scene_pkg(
        source.manifest.entry().unwrap()
    ));

    let backend = FakeUnpackBackend::new();
    backend.register(&package, synthetic_scene_pkg_entries());

    let task_dir = dir.path().join(".pkg2mpkg-unpack");
    let prepared = prepare_packaged_scene_source(
        &source,
        &backend,
        &task_dir,
        generous_limits(),
        scene_inventory_limits(),
    )
    .unwrap();

    assert_eq!(prepared.source.kind, WallpaperKind::Scene);
    assert_eq!(prepared.source.manifest.entry().unwrap(), "scene.json");
    assert!(!package_entry_is_raw_scene_pkg(
        prepared.source.manifest.entry().unwrap()
    ));
    assert!(
        prepared
            .tree
            .entries
            .iter()
            .any(|e| e.archive_path == "scene.json")
    );
    assert!(
        prepared
            .tree
            .entries
            .iter()
            .any(|e| e.archive_path == "project.json")
    );
    assert!(
        !prepared
            .tree
            .entries
            .iter()
            .any(|e| package_entry_is_raw_scene_pkg(&e.archive_path)),
        "raw scene.pkg must not remain in the prepared inventory"
    );
    assert!(task_dir.starts_with(dir.path()) || task_dir.exists());
}

#[test]
fn packaged_scene_does_not_select_project_manifest_as_scene_entry() {
    let error = prepare_native_scene_entry(&[(
        "project.json",
        br#"{"title":"embedded manifest","type":"scene","file":"scene.pkg"}"#,
    )])
    .unwrap_err();

    assert_eq!(error.code(), ErrorCode::ConversionFailed);
    assert!(error.to_string().contains("scene JSON"), "{error}");
}

#[test]
fn packaged_scene_requires_an_exact_json_suffix() {
    let selected = prepare_native_scene_entry(&[
        ("foo.json.bak", br#"{"objects":[]}"#),
        ("mobile_scene.json", br#"{"general":{},"objects":[]}"#),
    ])
    .unwrap();

    assert_eq!(selected, "mobile_scene.json");
}

#[test]
fn packaged_scene_ignores_task_sidecar_json() {
    let selected = prepare_native_scene_entry(&[
        (".pkg2mpkg-state.json", br#"{"objects":[]}"#),
        ("mobile_scene.json", br#"{"general":{},"objects":[]}"#),
    ])
    .unwrap();

    assert_eq!(selected, "mobile_scene.json");
}

#[test]
fn packaged_scene_rejects_arbitrary_top_level_json_without_scene_shape() {
    let error = prepare_native_scene_entry(&[(
        "metadata.json",
        br#"{"title":"not a scene","properties":{}}"#,
    )])
    .unwrap_err();

    assert_eq!(error.code(), ErrorCode::ConversionFailed);
    assert!(error.to_string().contains("scene JSON"), "{error}");
}

#[test]
fn packaged_scene_rejects_weak_objects_only_fallback_candidate() {
    let error = prepare_native_scene_entry(&[("metadata.json", br#"{"objects":[]}"#)]).unwrap_err();

    assert_eq!(error.code(), ErrorCode::ConversionFailed);
    assert!(error.to_string().contains("scene JSON"), "{error}");
}

#[test]
fn packaged_scene_rejects_invalid_exact_scene_json() {
    let error = prepare_native_scene_entry(&[(
        "scene.json",
        br#"{"title":"not scene data","type":"scene"}"#,
    )])
    .unwrap_err();

    assert_eq!(error.code(), ErrorCode::ConversionFailed);
    assert!(error.to_string().contains("scene JSON"), "{error}");
}

#[test]
fn packaged_scene_does_not_fallback_when_canonical_scene_json_is_invalid() {
    let error = prepare_native_scene_entry(&[
        ("scene.json", br#"{"title":"invalid canonical"}"#),
        ("mobile_scene.json", br#"{"general":{},"objects":[]}"#),
    ])
    .unwrap_err();

    assert_eq!(error.code(), ErrorCode::ConversionFailed);
    assert!(error.to_string().contains("scene.json"), "{error}");
    assert!(error.to_string().contains("invalid"), "{error}");
}

#[test]
fn packaged_scene_fails_closed_for_multiple_structurally_valid_candidates() {
    let error = prepare_native_scene_entry(&[
        ("alpha.json", br#"{"general":{},"objects":[]}"#),
        ("beta.json", br#"{"camera":{},"objects":[]}"#),
    ])
    .unwrap_err();

    assert_eq!(error.code(), ErrorCode::ConversionFailed);
    assert!(error.to_string().contains("ambiguous"), "{error}");
    assert!(error.to_string().contains("alpha.json"), "{error}");
    assert!(error.to_string().contains("beta.json"), "{error}");
}

#[test]
fn validate_unpacked_tree_rejects_raw_pkg_as_scene_entry() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Bad","type":"scene","file":"scene.pkg"}"#,
    );
    write_bytes(&dir.path().join("scene.pkg"), b"raw");
    let source = inspect_source(dir.path()).unwrap();
    let error = validate_unpacked_scene_tree(&source).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ConversionFailed);
}

#[cfg(unix)]
#[test]
fn checked_unpack_rejects_symlink_outputs() {
    use std::os::unix::fs::symlink;

    struct SymlinkBackend;

    impl ScenePackageUnpackBackend for SymlinkBackend {
        fn unpack_scene_package(
            &self,
            request: &ScenePackageUnpackRequest,
        ) -> pkg2mpkg_core::Result<ScenePackageUnpackReport> {
            fs::create_dir_all(&request.output_dir).unwrap();
            let target = request.output_dir.join("scene.json");
            write_bytes(&target, br#"{"general":{},"objects":[]}"#);
            let link = request.output_dir.join("escape.link");
            symlink("/etc/passwd", &link).unwrap();
            Ok(ScenePackageUnpackReport {
                output_dir: request.output_dir.clone(),
                entries: vec![
                    ScenePackageEntry {
                        path: "scene.json".into(),
                        size: 2,
                    },
                    ScenePackageEntry {
                        path: "escape.link".into(),
                        size: 0,
                    },
                ],
                total_bytes: 2,
            })
        }
    }

    let dir = tempdir().unwrap();
    let package = dir.path().join("scene.pkg");
    write_bytes(&package, b"pkg");
    let out = dir.path().join("out");
    let request = ScenePackageUnpackRequest::new(package, out, generous_limits()).unwrap();
    let error = unpack_scene_package_checked(&SymlinkBackend, &request).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProject);
    assert!(error.to_string().to_ascii_lowercase().contains("symlink"));
}

#[test]
fn native_backend_opens_pkgv_and_inventory_succeeds() {
    let dir = tempdir().unwrap();
    write_bytes(
        &dir.path().join("project.json"),
        br#"{"title":"Native PKGV","type":"scene","file":"scene.pkg","preview":"preview.jpg"}"#,
    );
    write_bytes(&dir.path().join("preview.jpg"), b"JPEG");
    let package = dir.path().join("scene.pkg");
    fs::write(
        &package,
        raw_pkg(
            "PKGV0001",
            &[
                ("scene.json", br#"{"general":{},"objects":[]}"#),
                ("materials/main.json", br#"{"passes":[]}"#),
                ("shaders/fx.frag", b"FRAG"),
            ],
        ),
    )
    .unwrap();

    // Desktop-only open API.
    let archive = DesktopPackageArchive::open(&package).unwrap();
    assert_eq!(archive.magic(), "PKGV0001");
    assert_eq!(archive.entries().len(), 3);

    let source = inspect_source(dir.path()).unwrap();
    let task_dir = dir.path().join(".pkg2mpkg-unpack");
    let prepared = prepare_packaged_scene_source(
        &source,
        &NativeScenePackageUnpackBackend::new(),
        &task_dir,
        generous_limits(),
        scene_inventory_limits(),
    )
    .unwrap();

    assert_eq!(prepared.source.manifest.entry().unwrap(), "scene.json");
    assert!(
        prepared
            .tree
            .entries
            .iter()
            .any(|e| e.archive_path == "materials/main.json")
    );
    assert!(
        !prepared
            .tree
            .entries
            .iter()
            .any(|e| package_entry_is_raw_scene_pkg(&e.archive_path))
    );
    assert!(task_dir.join("scene.json").is_file());
    assert!(task_dir.join("materials/main.json").is_file());
}

#[test]
fn native_backend_rejects_pkgm_bytes_as_desktop_package() {
    let dir = tempdir().unwrap();
    let package = dir.path().join("scene.pkg");
    // Same layout, wrong magic for the desktop-only API.
    fs::write(
        &package,
        raw_pkg("PKGM0020", &[("scene.json", br#"{"objects":[]}"#)]),
    )
    .unwrap();
    let out = dir.path().join("out");
    let request = ScenePackageUnpackRequest::new(package, out, generous_limits()).unwrap();
    let error = NativeScenePackageUnpackBackend::new()
        .unpack_scene_package(&request)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProject);
    assert!(
        error.to_string().contains("PKGV"),
        "expected PKGV requirement, got: {error}"
    );
}

#[test]
fn native_backend_respects_size_limits() {
    let dir = tempdir().unwrap();
    let package = dir.path().join("scene.pkg");
    fs::write(
        &package,
        raw_pkg(
            "PKGV0001",
            &[
                ("scene.json", br#"{"objects":[]}"#),
                ("big.bin", &[b'x'; 100]),
            ],
        ),
    )
    .unwrap();
    let out = dir.path().join("out");
    let request = ScenePackageUnpackRequest::new(
        package,
        out,
        ScenePackageUnpackLimits {
            max_entries: 10,
            max_path_length: 16_384,
            max_file_bytes: 50,
            max_total_bytes: 10_000,
        },
    )
    .unwrap();
    let error = NativeScenePackageUnpackBackend::new()
        .unpack_scene_package(&request)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProject);
    assert!(
        error.to_string().to_ascii_lowercase().contains("size")
            || error.to_string().to_ascii_lowercase().contains("limit"),
        "{error}"
    );
}

/// Backend that returns a fixed report, optionally writing files.
struct ReportingBackend {
    entries: Vec<ScenePackageEntry>,
    write: bool,
}

impl ScenePackageUnpackBackend for ReportingBackend {
    fn unpack_scene_package(
        &self,
        request: &ScenePackageUnpackRequest,
    ) -> pkg2mpkg_core::Result<ScenePackageUnpackReport> {
        if self.write {
            fs::create_dir_all(&request.output_dir).unwrap();
            for entry in &self.entries {
                let dest = request.output_dir.join(&entry.path);
                if let Some(parent) = dest.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::write(&dest, vec![b'x'; entry.size as usize]);
            }
        }
        let total_bytes = self.entries.iter().map(|e| e.size).sum();
        Ok(ScenePackageUnpackReport {
            output_dir: request.output_dir.clone(),
            entries: self.entries.clone(),
            total_bytes,
        })
    }
}
