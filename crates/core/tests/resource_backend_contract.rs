use std::path::{Path, PathBuf};

use pkg2mpkg_core::{
    Compression, ErrorCode, Reduction, ResourceTranscodeBackend, TextureTranscodeReport,
    TextureTranscodeRequest, transcode_texture_checked,
};

struct ReportingBackend {
    reported_output: PathBuf,
}

impl ResourceTranscodeBackend for ReportingBackend {
    fn transcode_texture(
        &self,
        request: &TextureTranscodeRequest,
    ) -> pkg2mpkg_core::Result<TextureTranscodeReport> {
        Ok(TextureTranscodeReport {
            output: self.reported_output.clone(),
            input_bytes: 128,
            output_bytes: 64,
            compression: request.compression,
            reduction: request.reduction,
        })
    }
}

#[test]
fn texture_request_locks_the_windows_max_mipmap_policy() {
    let request = TextureTranscodeRequest::new(
        PathBuf::from("source.tex"),
        PathBuf::from("converted.tex"),
        Compression::HighQuality,
        Reduction::X2,
    )
    .unwrap();

    assert_eq!(request.input, Path::new("source.tex"));
    assert_eq!(request.output, Path::new("converted.tex"));
    assert_eq!(request.compression, Compression::HighQuality);
    assert_eq!(request.reduction, Reduction::X2);
    assert_eq!(request.max_mipmaps, 1);
}

#[test]
fn checked_backend_accepts_only_the_requested_output() {
    let request = TextureTranscodeRequest::new(
        PathBuf::from("source.tex"),
        PathBuf::from("converted.tex"),
        Compression::HighPerformance,
        Reduction::Original,
    )
    .unwrap();
    let backend = ReportingBackend {
        reported_output: request.output.clone(),
    };

    let report = transcode_texture_checked(&backend, &request).unwrap();
    assert_eq!(report.output, request.output);
    assert_eq!(report.output_bytes, 64);
}

#[test]
fn checked_backend_rejects_a_different_reported_output() {
    let request = TextureTranscodeRequest::new(
        PathBuf::from("source.tex"),
        PathBuf::from("converted.tex"),
        Compression::HighPerformance,
        Reduction::X4,
    )
    .unwrap();
    let backend = ReportingBackend {
        reported_output: PathBuf::from("different.tex"),
    };

    let error = transcode_texture_checked(&backend, &request).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ConversionFailed);
    assert!(error.to_string().contains("different.tex"));
    assert!(error.to_string().contains("converted.tex"));
}

#[test]
fn texture_request_rejects_same_input_and_output() {
    let error = TextureTranscodeRequest::new(
        PathBuf::from("same.tex"),
        PathBuf::from("same.tex"),
        Compression::HighQuality,
        Reduction::Original,
    )
    .unwrap_err();

    assert_eq!(error.code(), ErrorCode::InvalidArguments);
}
