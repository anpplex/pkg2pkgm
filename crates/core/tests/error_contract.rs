use pkg2mpkg_core::{Error, ErrorCode, Stage};

#[test]
fn unsupported_type_has_stable_contract() {
    let error = Error::unsupported_type("web");
    assert_eq!(error.code(), ErrorCode::UnsupportedWallpaperType);
    assert_eq!(error.stage(), Stage::Inspect);
    assert_eq!(error.code().exit_code(), 3);
    assert!(error.to_string().contains("web"));
}

#[test]
fn invalid_package_maps_to_exit_code_four() {
    let error = Error::invalid_mpkg("entry range exceeds file");
    assert_eq!(error.code(), ErrorCode::InvalidMpkg);
    assert_eq!(error.stage(), Stage::Verify);
    assert_eq!(error.code().exit_code(), 4);
}
