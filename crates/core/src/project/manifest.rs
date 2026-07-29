use serde_json::Value;

use crate::{Error, Result};

use super::WallpaperKind;

#[derive(Debug, Clone)]
pub struct ProjectManifest {
    raw: Value,
}

impl ProjectManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let raw: Value = serde_json::from_slice(bytes).map_err(|source| Error::InvalidProject {
            reason: format!("project.json is not valid JSON: {source}"),
        })?;
        if !raw.is_object() {
            return Err(Error::InvalidProject {
                reason: "project.json root must be an object".into(),
            });
        }
        Ok(Self { raw })
    }

    pub fn title(&self) -> Option<&str> {
        self.raw.get("title").and_then(Value::as_str)
    }

    pub fn entry(&self) -> Option<&str> {
        self.raw.get("file").and_then(Value::as_str)
    }

    pub fn kind(&self) -> Result<WallpaperKind> {
        if let Some(kind) = self.declared_kind()? {
            return Ok(kind);
        }
        let entry = self.entry().ok_or_else(|| Error::InvalidProject {
            reason: "project.json is missing a string file field".into(),
        })?;
        WallpaperKind::infer_from_entry(entry).ok_or_else(|| Error::InvalidProject {
            reason: format!("cannot infer wallpaper type from entry: {entry}"),
        })
    }

    pub fn raw(&self) -> &Value {
        &self.raw
    }

    pub fn into_raw(self) -> Value {
        self.raw
    }

    pub(crate) fn declared_kind(&self) -> Result<Option<WallpaperKind>> {
        match self.raw.get("type") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(value)) => WallpaperKind::parse_declared(value).map(Some),
            Some(_) => Err(Error::InvalidProject {
                reason: "project.json type must be a string".into(),
            }),
        }
    }
}
