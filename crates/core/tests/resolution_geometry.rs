use pkg2mpkg_core::{Alignment, CropMode, CropRect, Dimensions, ErrorCode, resolve_video_geometry};

#[test]
fn cover_uses_the_exact_landscape_car_canvas() {
    let geometry = resolve_video_geometry(
        Dimensions::new(1080, 1920).unwrap(),
        Dimensions::new_h264(1920, 1080).unwrap(),
        CropMode::Cover,
        Alignment::CENTER,
    )
    .unwrap();
    assert_eq!(geometry.output, Dimensions::new_h264(1920, 1080).unwrap());
    assert!(geometry.crop.is_some());
}

#[test]
fn centered_square_cover_has_a_hand_checked_source_crop() {
    let geometry = resolve_video_geometry(
        Dimensions::new(1920, 1080).unwrap(),
        Dimensions::new_h264(1000, 1000).unwrap(),
        CropMode::Cover,
        Alignment::CENTER,
    )
    .unwrap();
    assert_eq!(
        geometry.crop,
        Some(CropRect {
            x: 420,
            y: 0,
            width: 1080,
            height: 1080,
        })
    );
}

#[test]
fn cover_alignment_accepts_both_edges() {
    let source = Dimensions::new(1920, 1080).unwrap();
    let target = Dimensions::new_h264(1000, 1000).unwrap();
    let left = resolve_video_geometry(
        source,
        target,
        CropMode::Cover,
        Alignment::new(0, 50).unwrap(),
    )
    .unwrap();
    let right = resolve_video_geometry(
        source,
        target,
        CropMode::Cover,
        Alignment::new(100, 50).unwrap(),
    )
    .unwrap();
    assert_eq!(left.crop.unwrap().x, 0);
    assert_eq!(right.crop.unwrap().x, 840);
}

#[test]
fn keep_aspect_uses_exact_as_a_boundary_without_padding() {
    let geometry = resolve_video_geometry(
        Dimensions::new(1920, 1080).unwrap(),
        Dimensions::new_h264(1000, 1000).unwrap(),
        CropMode::KeepAspect,
        Alignment::CENTER,
    )
    .unwrap();
    assert_eq!(geometry.output, Dimensions::new(1000, 562).unwrap());
    assert!(geometry.crop.is_none());
}

#[test]
fn matching_aspect_cover_does_not_invent_a_crop() {
    let geometry = resolve_video_geometry(
        Dimensions::new(1920, 1080).unwrap(),
        Dimensions::new_h264(1280, 720).unwrap(),
        CropMode::Cover,
        Alignment::CENTER,
    )
    .unwrap();
    assert_eq!(geometry.crop, None);
}

#[test]
fn odd_source_is_allowed_but_odd_h264_target_is_rejected() {
    let source = Dimensions::new(1919, 1080).unwrap();
    assert_eq!(source.width, 1919);

    let error = Dimensions::new_h264(1919, 1080).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArguments);
    assert!(error.to_string().contains("1918"));
    assert!(error.to_string().contains("1920"));
}

#[test]
fn zero_dimensions_and_out_of_range_alignment_are_rejected() {
    assert_eq!(
        Dimensions::new(0, 1080).unwrap_err().code(),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        Alignment::new(101, 50).unwrap_err().code(),
        ErrorCode::InvalidArguments
    );
}
