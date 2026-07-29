use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WallpaperKind {
    Scene,
    Video,
    Web,
    Application,
}

impl WallpaperKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scene => "scene",
            Self::Video => "video",
            Self::Web => "web",
            Self::Application => "application",
        }
    }

    pub(crate) fn parse_declared(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "scene" => Ok(Self::Scene),
            "video" => Ok(Self::Video),
            "web" => Ok(Self::Web),
            "application" => Ok(Self::Application),
            _ => Err(Error::InvalidProject {
                reason: format!("unknown wallpaper type: {value}"),
            }),
        }
    }

    pub(crate) fn infer_from_entry(entry: &str) -> Option<Self> {
        let extension = std::path::Path::new(entry)
            .extension()
            .and_then(|value| value.to_str())?
            .to_ascii_lowercase();
        match extension.as_str() {
            "html" | "htm" => Some(Self::Web),
            "exe" => Some(Self::Application),
            "mp4" | "webm" => Some(Self::Video),
            "pkg" => Some(Self::Scene),
            _ => None,
        }
    }
}
