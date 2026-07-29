use std::{io, path::PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Arguments,
    Inspect,
    Plan,
    Unpack,
    Convert,
    Pack,
    Verify,
    Device,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidArguments,
    UnsupportedWallpaperType,
    InvalidProject,
    InvalidMpkg,
    BackendUnavailable,
    ConversionFailed,
    OutputIo,
    PackageTooLarge,
    VerificationFailed,
    DeviceFailed,
    Cancelled,
}

impl ErrorCode {
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::InvalidArguments => 2,
            Self::UnsupportedWallpaperType => 3,
            Self::InvalidProject | Self::InvalidMpkg => 4,
            Self::BackendUnavailable => 5,
            Self::ConversionFailed => 6,
            Self::OutputIo | Self::PackageTooLarge => 7,
            Self::VerificationFailed => 8,
            Self::DeviceFailed => 9,
            Self::Cancelled => 130,
        }
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid arguments: {reason}")]
    InvalidArguments { reason: String },
    #[error("unsupported wallpaper type: {kind}")]
    UnsupportedWallpaperType { kind: String },
    #[error("invalid project: {reason}")]
    InvalidProject { reason: String },
    #[error("invalid MPKG: {reason}")]
    InvalidMpkg { reason: String },
    #[error("I/O failure during {stage:?} at {path}: {source}")]
    Io {
        stage: Stage,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("backend unavailable: {backend}")]
    BackendUnavailable { backend: String },
    #[error("conversion failed: {reason}")]
    ConversionFailed { reason: String },
    #[error("package size {size} exceeds the 4 GiB limit")]
    PackageTooLarge { size: u64 },
    #[error("verification failed: {reason}")]
    VerificationFailed { reason: String },
    #[error("device operation failed: {reason}")]
    DeviceFailed { reason: String },
    #[error("operation cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn unsupported_type(kind: impl Into<String>) -> Self {
        Self::UnsupportedWallpaperType { kind: kind.into() }
    }

    pub fn invalid_mpkg(reason: impl Into<String>) -> Self {
        Self::InvalidMpkg {
            reason: reason.into(),
        }
    }

    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidArguments { .. } => ErrorCode::InvalidArguments,
            Self::UnsupportedWallpaperType { .. } => ErrorCode::UnsupportedWallpaperType,
            Self::InvalidProject { .. } => ErrorCode::InvalidProject,
            Self::InvalidMpkg { .. } => ErrorCode::InvalidMpkg,
            Self::Io { stage, .. } => match stage {
                Stage::Arguments | Stage::Plan => ErrorCode::InvalidArguments,
                Stage::Inspect | Stage::Unpack => ErrorCode::InvalidProject,
                Stage::Convert => ErrorCode::ConversionFailed,
                Stage::Pack => ErrorCode::OutputIo,
                Stage::Verify => ErrorCode::InvalidMpkg,
                Stage::Device => ErrorCode::DeviceFailed,
            },
            Self::BackendUnavailable { .. } => ErrorCode::BackendUnavailable,
            Self::ConversionFailed { .. } => ErrorCode::ConversionFailed,
            Self::PackageTooLarge { .. } => ErrorCode::PackageTooLarge,
            Self::VerificationFailed { .. } => ErrorCode::VerificationFailed,
            Self::DeviceFailed { .. } => ErrorCode::DeviceFailed,
            Self::Cancelled => ErrorCode::Cancelled,
        }
    }

    pub const fn stage(&self) -> Stage {
        match self {
            Self::InvalidArguments { .. } => Stage::Arguments,
            Self::UnsupportedWallpaperType { .. } | Self::InvalidProject { .. } => Stage::Inspect,
            Self::BackendUnavailable { .. } | Self::Cancelled => Stage::Plan,
            Self::ConversionFailed { .. } => Stage::Convert,
            Self::Io { stage, .. } => *stage,
            Self::PackageTooLarge { .. } => Stage::Pack,
            Self::InvalidMpkg { .. } | Self::VerificationFailed { .. } => Stage::Verify,
            Self::DeviceFailed { .. } => Stage::Device,
        }
    }
}
