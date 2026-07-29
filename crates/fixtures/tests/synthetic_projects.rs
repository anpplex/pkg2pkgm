use pkg2mpkg_core::{ErrorCode, WallpaperKind, inspect_source};
use pkg2mpkg_fixtures::{
    synthetic_application_project, synthetic_scene_project, synthetic_video_project,
    synthetic_web_project,
};

#[test]
fn scene_and_video_fixtures_are_valid_source_projects() {
    let scene = synthetic_scene_project();
    let video = synthetic_video_project();
    assert_eq!(
        inspect_source(scene.path()).unwrap().kind,
        WallpaperKind::Scene
    );
    assert_eq!(
        inspect_source(video.path()).unwrap().kind,
        WallpaperKind::Video
    );
}

#[test]
fn web_and_application_fixtures_hit_the_unsupported_gate() {
    for project in [synthetic_web_project(), synthetic_application_project()] {
        assert_eq!(
            inspect_source(project.path()).unwrap_err().code(),
            ErrorCode::UnsupportedWallpaperType
        );
    }
}
