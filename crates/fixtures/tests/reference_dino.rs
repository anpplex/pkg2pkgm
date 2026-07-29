use std::{env, path::Path};

use pkg2mpkg_core::{ContainerVersion, MpkgArchive, ProjectManifest, WallpaperKind};

#[test]
#[ignore = "requires WE_DINO_MPKG extracted from the locally installed official APK"]
fn official_android_dino_is_v18_scene() {
    let path = env::var("WE_DINO_MPKG").expect("set WE_DINO_MPKG");
    let archive = MpkgArchive::open(Path::new(&path)).unwrap();
    assert_eq!(archive.version(), ContainerVersion::Pkgm0018);
    let project = archive.read_entry("project.json").unwrap();
    let manifest = ProjectManifest::parse(&project).unwrap();
    assert_eq!(manifest.kind().unwrap(), WallpaperKind::Scene);
    assert_eq!(manifest.title(), Some("Dino Run"));
}
