use pkg2mpkg_core::{
    Compression, ContentClass, Reduction, SceneMode, SceneProfile, resolve_scene_profile,
};

#[test]
fn matches_windows_2826_dynamic_matrix() {
    let cases = [
        (
            SceneProfile::High,
            ContentClass::PixelArt,
            Compression::HighQuality,
            Reduction::Original,
        ),
        (
            SceneProfile::High,
            ContentClass::Normal,
            Compression::HighPerformance,
            Reduction::Original,
        ),
        (
            SceneProfile::High,
            ContentClass::Uhd,
            Compression::HighPerformance,
            Reduction::X2,
        ),
        (
            SceneProfile::Balanced,
            ContentClass::PixelArt,
            Compression::HighPerformance,
            Reduction::Original,
        ),
        (
            SceneProfile::Balanced,
            ContentClass::Normal,
            Compression::HighPerformance,
            Reduction::X2,
        ),
        (
            SceneProfile::Balanced,
            ContentClass::Uhd,
            Compression::HighPerformance,
            Reduction::X4,
        ),
    ];

    for (profile, class, compression, reduction) in cases {
        assert_eq!(
            resolve_scene_profile(profile, class),
            SceneMode::Dynamic {
                compression,
                reduction
            }
        );
    }
    assert_eq!(
        resolve_scene_profile(SceneProfile::Performance, ContentClass::Normal),
        SceneMode::PreRendered
    );
}

#[test]
fn custom_profile_uses_explicit_values_for_every_content_class() {
    let custom = SceneProfile::Custom {
        compression: Compression::HighQuality,
        reduction: Reduction::X4,
    };
    for class in [
        ContentClass::PixelArt,
        ContentClass::Normal,
        ContentClass::Uhd,
    ] {
        assert_eq!(
            resolve_scene_profile(custom, class),
            SceneMode::Dynamic {
                compression: Compression::HighQuality,
                reduction: Reduction::X4,
            }
        );
    }
}

#[test]
fn profile_wire_values_match_the_windows_export_options() {
    assert_eq!(
        serde_json::to_string(&Compression::HighPerformance).unwrap(),
        r#""high_performance""#
    );
    assert_eq!(
        serde_json::to_string(&Reduction::Original).unwrap(),
        r#""high_quality""#
    );
    assert_eq!(
        serde_json::to_string(&Reduction::X2).unwrap(),
        r#""reduction_x2""#
    );
}
