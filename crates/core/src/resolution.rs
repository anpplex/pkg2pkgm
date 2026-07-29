use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
}

impl Dimensions {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(invalid_arguments(format!(
                "dimensions must be positive, got {width}x{height}"
            )));
        }
        Ok(Self { width, height })
    }

    pub fn new_h264(width: u32, height: u32) -> Result<Self> {
        let dimensions = Self::new(width, height)?;
        if width % 2 != 0 || height % 2 != 0 {
            return Err(invalid_arguments(format!(
                "H.264 dimensions must be even, got {width}x{height}; valid neighbors are {}x{} and {}x{}",
                width.saturating_sub(width % 2),
                height.saturating_sub(height % 2),
                width
                    .checked_add(width % 2)
                    .unwrap_or(width.saturating_sub(1)),
                height
                    .checked_add(height % 2)
                    .unwrap_or(height.saturating_sub(1)),
            )));
        }
        Ok(dimensions)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CropMode {
    Cover,
    KeepAspect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alignment {
    pub x: u8,
    pub y: u8,
}

impl Alignment {
    pub const CENTER: Self = Self { x: 50, y: 50 };

    pub fn new(x: u8, y: u8) -> Result<Self> {
        if x > 100 || y > 100 {
            return Err(invalid_arguments(format!(
                "alignment must be between 0 and 100, got {x},{y}"
            )));
        }
        Ok(Self { x, y })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedVideoGeometry {
    pub output: Dimensions,
    pub crop: Option<CropRect>,
}

pub fn resolve_video_geometry(
    source: Dimensions,
    boundary: Dimensions,
    crop_mode: CropMode,
    alignment: Alignment,
) -> Result<ResolvedVideoGeometry> {
    let source = Dimensions::new(source.width, source.height)?;
    let boundary = Dimensions::new_h264(boundary.width, boundary.height)?;
    let alignment = Alignment::new(alignment.x, alignment.y)?;

    match crop_mode {
        CropMode::KeepAspect => fit_inside(source, boundary),
        CropMode::Cover => cover(source, boundary, alignment),
    }
}

fn fit_inside(source: Dimensions, boundary: Dimensions) -> Result<ResolvedVideoGeometry> {
    let source_cross = checked_product(source.width, boundary.height)?;
    let boundary_cross = checked_product(source.height, boundary.width)?;
    let (width, height) = if source_cross >= boundary_cross {
        (
            boundary.width,
            even_floor(checked_scale(source.height, boundary.width, source.width)?)?,
        )
    } else {
        (
            even_floor(checked_scale(source.width, boundary.height, source.height)?)?,
            boundary.height,
        )
    };

    Ok(ResolvedVideoGeometry {
        output: Dimensions::new_h264(width, height)?,
        crop: None,
    })
}

fn cover(
    source: Dimensions,
    boundary: Dimensions,
    alignment: Alignment,
) -> Result<ResolvedVideoGeometry> {
    let source_cross = checked_product(source.width, boundary.height)?;
    let boundary_cross = checked_product(source.height, boundary.width)?;
    let crop = if source_cross == boundary_cross {
        None
    } else if source_cross > boundary_cross {
        let width = checked_scale(source.height, boundary.width, boundary.height)?;
        if width == 0 || width > source.width {
            return Err(invalid_arguments("cannot compute a valid horizontal crop"));
        }
        let remaining = source.width - width;
        Some(CropRect {
            x: aligned_offset(remaining, alignment.x)?,
            y: 0,
            width,
            height: source.height,
        })
    } else {
        let height = checked_scale(source.width, boundary.height, boundary.width)?;
        if height == 0 || height > source.height {
            return Err(invalid_arguments("cannot compute a valid vertical crop"));
        }
        let remaining = source.height - height;
        Some(CropRect {
            x: 0,
            y: aligned_offset(remaining, alignment.y)?,
            width: source.width,
            height,
        })
    };

    Ok(ResolvedVideoGeometry {
        output: boundary,
        crop,
    })
}

fn checked_product(left: u32, right: u32) -> Result<u64> {
    u64::from(left)
        .checked_mul(u64::from(right))
        .ok_or_else(|| invalid_arguments("dimension arithmetic overflow"))
}

fn checked_scale(value: u32, numerator: u32, denominator: u32) -> Result<u32> {
    let scaled = checked_product(value, numerator)? / u64::from(denominator);
    u32::try_from(scaled).map_err(|_| invalid_arguments("scaled dimension exceeds u32"))
}

fn even_floor(value: u32) -> Result<u32> {
    let even = value - value % 2;
    if even == 0 {
        Err(invalid_arguments(
            "scaled H.264 dimension would be smaller than 2 pixels",
        ))
    } else {
        Ok(even)
    }
}

fn aligned_offset(remaining: u32, alignment: u8) -> Result<u32> {
    checked_scale(remaining, u32::from(alignment), 100)
}

fn invalid_arguments(reason: impl Into<String>) -> Error {
    Error::InvalidArguments {
        reason: reason.into(),
    }
}
