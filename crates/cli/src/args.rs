use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use pkg2mpkg_core::{Compression, Reduction, SceneProfile};

#[derive(Debug, Parser)]
#[command(
    name = "pkg2mpkg",
    version,
    about = "Cross-platform Wallpaper Engine mobile package tool"
)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Inspect {
        input: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Verify {
        input: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Export {
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, value_enum)]
        profile: Option<ProfileArg>,
        #[arg(long, value_enum)]
        compression: Option<CompressionArg>,
        #[arg(long, value_enum)]
        reduction: Option<ReductionArg>,
        #[arg(long = "we-runtime", value_name = "DIR")]
        we_runtime: Option<PathBuf>,
        #[arg(long, value_name = "PATH")]
        wine: Option<PathBuf>,
        #[arg(long, value_name = "PATH")]
        winepath: Option<PathBuf>,
        #[arg(long)]
        replace: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
}

impl Command {
    pub const fn json(&self) -> bool {
        match self {
            Self::Inspect { json, .. } | Self::Verify { json, .. } | Self::Export { json, .. } => {
                *json
            }
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ProfileArg {
    High,
    Balanced,
    Performance,
    Custom,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CompressionArg {
    HighQuality,
    HighPerformance,
}

impl From<CompressionArg> for Compression {
    fn from(value: CompressionArg) -> Self {
        match value {
            CompressionArg::HighQuality => Self::HighQuality,
            CompressionArg::HighPerformance => Self::HighPerformance,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ReductionArg {
    HighQuality,
    ReductionX2,
    ReductionX4,
}

impl From<ReductionArg> for Reduction {
    fn from(value: ReductionArg) -> Self {
        match value {
            ReductionArg::HighQuality => Self::Original,
            ReductionArg::ReductionX2 => Self::X2,
            ReductionArg::ReductionX4 => Self::X4,
        }
    }
}

/// Resolve the Scene profile from CLI flags after validating custom pairings.
pub fn resolve_scene_profile_arg(
    profile: ProfileArg,
    compression: Option<CompressionArg>,
    reduction: Option<ReductionArg>,
) -> Result<SceneProfile, String> {
    match profile {
        ProfileArg::High => {
            reject_custom_only_flags(compression, reduction, "high")?;
            Ok(SceneProfile::High)
        }
        ProfileArg::Balanced => {
            reject_custom_only_flags(compression, reduction, "balanced")?;
            Ok(SceneProfile::Balanced)
        }
        ProfileArg::Performance => {
            reject_custom_only_flags(compression, reduction, "performance")?;
            Ok(SceneProfile::Performance)
        }
        ProfileArg::Custom => match (compression, reduction) {
            (Some(compression), Some(reduction)) => Ok(SceneProfile::Custom {
                compression: compression.into(),
                reduction: reduction.into(),
            }),
            _ => Err("custom profile requires both --compression and --reduction".into()),
        },
    }
}

fn reject_custom_only_flags(
    compression: Option<CompressionArg>,
    reduction: Option<ReductionArg>,
    profile: &str,
) -> Result<(), String> {
    if compression.is_some() || reduction.is_some() {
        return Err(format!(
            "--compression and --reduction are only valid with --profile custom (not {profile})"
        ));
    }
    Ok(())
}
