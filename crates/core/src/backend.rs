use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Compression, Error, Reduction, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperRequirement {
    ResourceTranscode,
    SceneCapture,
    H264Encode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendCapabilities {
    pub protocol_version: u32,
    pub requirements: Vec<HelperRequirement>,
}

impl BackendCapabilities {
    pub fn satisfies(&self, requirement: &HelperRequirement) -> bool {
        self.requirements.contains(requirement)
    }

    pub fn missing_requirements(&self, required: &[HelperRequirement]) -> Vec<HelperRequirement> {
        required
            .iter()
            .copied()
            .filter(|requirement| !self.satisfies(requirement))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextureTranscodeRequest {
    pub input: PathBuf,
    pub output: PathBuf,
    pub compression: Compression,
    pub reduction: Reduction,
    pub max_mipmaps: u32,
}

impl TextureTranscodeRequest {
    pub fn new(
        input: PathBuf,
        output: PathBuf,
        compression: Compression,
        reduction: Reduction,
    ) -> Result<Self> {
        if input.as_os_str().is_empty() || output.as_os_str().is_empty() {
            return Err(Error::InvalidArguments {
                reason: "texture input and output paths must not be empty".into(),
            });
        }
        if input == output {
            return Err(Error::InvalidArguments {
                reason: format!("texture input and output must differ: {}", input.display()),
            });
        }
        Ok(Self {
            input,
            output,
            compression,
            reduction,
            max_mipmaps: 1,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextureTranscodeReport {
    pub output: PathBuf,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub compression: Compression,
    pub reduction: Reduction,
}

pub trait ResourceTranscodeBackend: Send + Sync {
    fn transcode_texture(
        &self,
        request: &TextureTranscodeRequest,
    ) -> Result<TextureTranscodeReport>;
}

pub fn transcode_texture_checked(
    backend: &dyn ResourceTranscodeBackend,
    request: &TextureTranscodeRequest,
) -> Result<TextureTranscodeReport> {
    let report = backend.transcode_texture(request)?;
    if report.output != request.output {
        return Err(Error::ConversionFailed {
            reason: format!(
                "texture backend reported output {} instead of {}",
                report.output.display(),
                request.output.display()
            ),
        });
    }
    if report.compression != request.compression || report.reduction != request.reduction {
        return Err(Error::ConversionFailed {
            reason: "texture backend reported settings that differ from the request".into(),
        });
    }
    Ok(report)
}
