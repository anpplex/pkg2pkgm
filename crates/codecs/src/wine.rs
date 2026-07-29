use std::{ffi::OsString, path::Path, path::PathBuf, process::Command};

use pkg2mpkg_core::{Error, Result};

use crate::resource_compiler::{describe_process_failure, run_bounded};

#[derive(Debug, Clone)]
pub(crate) struct WineLauncher {
    wine: PathBuf,
    winepath: PathBuf,
}

impl WineLauncher {
    pub(crate) fn new(wine: impl Into<PathBuf>, winepath: impl Into<PathBuf>) -> Self {
        Self {
            wine: wine.into(),
            winepath: winepath.into(),
        }
    }

    pub(crate) fn wine(&self) -> &Path {
        &self.wine
    }

    pub(crate) fn winepath(&self) -> &Path {
        &self.winepath
    }

    pub(crate) fn translate_paths(
        &self,
        compiler: &Path,
        input: &Path,
        output: &Path,
    ) -> Result<TranslatedPaths> {
        Ok(TranslatedPaths {
            compiler: self.translate_path(compiler, "resource compiler")?,
            input: self.translate_path(input, "texture input")?,
            output: self.translate_path(output, "texture output")?,
        })
    }

    fn translate_path(&self, path: &Path, label: &str) -> Result<OsString> {
        let mut command = Command::new(&self.winepath);
        command.arg("-w").arg(path);
        let process =
            run_bounded(command).map_err(|error| error.into_error("winepath", &self.winepath))?;

        if !process.status.success() {
            return Err(Error::ConversionFailed {
                reason: format!(
                    "could not translate {label} {}; {}",
                    path.display(),
                    describe_process_failure("winepath", &process)
                ),
            });
        }
        if process.stdout.truncated() {
            return Err(Error::ConversionFailed {
                reason: format!(
                    "winepath returned an overlong Windows path for {label} {}",
                    path.display()
                ),
            });
        }

        let mut bytes = process.stdout.bytes().to_vec();
        while matches!(bytes.last(), Some(b'\n' | b'\r')) {
            bytes.pop();
        }
        if bytes.is_empty() || bytes.contains(&b'\n') || bytes.contains(&b'\r') {
            return Err(Error::ConversionFailed {
                reason: format!(
                    "winepath returned a suspicious Windows path for {label} {}",
                    path.display()
                ),
            });
        }

        let translated = String::from_utf8(bytes).map_err(|_| Error::ConversionFailed {
            reason: format!(
                "winepath returned a non-UTF-8 Windows path for {label} {}",
                path.display()
            ),
        })?;
        Ok(OsString::from(translated))
    }
}

pub(crate) struct TranslatedPaths {
    pub(crate) compiler: OsString,
    pub(crate) input: OsString,
    pub(crate) output: OsString,
}
