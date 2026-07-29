use std::{
    ffi::OsStr,
    fs::{self, File, Metadata},
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
};

use pkg2mpkg_core::{
    Compression, Error, Reduction, ResourceTranscodeBackend, Result, TextureTranscodeReport,
    TextureTranscodeRequest,
};
use tempfile::TempPath;

#[cfg(not(windows))]
use crate::wine::WineLauncher;

const TEX_V5_MAGIC: &[u8; 9] = b"TEXV0005\0";
const DIAGNOSTIC_LIMIT: usize = 8 * 1024;

#[derive(Debug, Clone)]
pub struct ResourceCompilerBackend {
    compiler: PathBuf,
    launch_mode: LaunchMode,
}

#[derive(Debug, Clone)]
enum LaunchMode {
    #[cfg(windows)]
    Native,
    #[cfg(not(windows))]
    Wine(WineLauncher),
}

impl ResourceCompilerBackend {
    #[cfg(windows)]
    pub fn native(compiler: impl Into<PathBuf>) -> Self {
        Self {
            compiler: compiler.into(),
            launch_mode: LaunchMode::Native,
        }
    }

    #[cfg(not(windows))]
    pub fn wine(
        compiler: impl Into<PathBuf>,
        wine: impl Into<PathBuf>,
        winepath: impl Into<PathBuf>,
    ) -> Self {
        Self {
            compiler: compiler.into(),
            launch_mode: LaunchMode::Wine(WineLauncher::new(wine, winepath)),
        }
    }

    fn validate_request(&self, request: &TextureTranscodeRequest) -> Result<u64> {
        if request.max_mipmaps != 1 {
            return Err(Error::InvalidArguments {
                reason: format!(
                    "resource compiler requires max_mipmaps 1, got {}",
                    request.max_mipmaps
                ),
            });
        }

        validate_backend_file(&self.compiler, "resource compiler")?;
        #[cfg(not(windows))]
        let LaunchMode::Wine(launcher) = &self.launch_mode;
        #[cfg(not(windows))]
        {
            validate_backend_file(launcher.wine(), "Wine runtime")?;
            validate_backend_file(launcher.winepath(), "winepath")?;
        }

        let input_metadata = validate_input_file(&request.input)?;
        validate_output_is_absent(&request.input, &request.output)?;

        Ok(input_metadata.len())
    }

    fn command(&self, request: &TextureTranscodeRequest, output: &Path) -> Result<Command> {
        match &self.launch_mode {
            #[cfg(windows)]
            LaunchMode::Native => {
                let mut command = Command::new(&self.compiler);
                append_compiler_arguments(
                    &mut command,
                    request.input.as_os_str(),
                    output.as_os_str(),
                    request,
                );
                Ok(command)
            }
            #[cfg(not(windows))]
            LaunchMode::Wine(launcher) => {
                let paths = launcher.translate_paths(&self.compiler, &request.input, output)?;
                let mut command = Command::new(launcher.wine());
                command.arg(paths.compiler);
                append_compiler_arguments(&mut command, &paths.input, &paths.output, request);
                Ok(command)
            }
        }
    }

    fn process_name(&self) -> (&'static str, &Path) {
        match &self.launch_mode {
            #[cfg(windows)]
            LaunchMode::Native => ("resource compiler", &self.compiler),
            #[cfg(not(windows))]
            LaunchMode::Wine(launcher) => ("Wine runtime", launcher.wine()),
        }
    }
}

impl ResourceTranscodeBackend for ResourceCompilerBackend {
    fn transcode_texture(
        &self,
        request: &TextureTranscodeRequest,
    ) -> Result<TextureTranscodeReport> {
        let input_bytes = self.validate_request(request)?;
        let temporary_output = create_owned_temporary_output(&request.output)?;

        let conversion = (|| -> Result<Metadata> {
            let command = self.command(request, temporary_output.as_ref())?;
            let (process_name, process_path) = self.process_name();
            let process = run_bounded(command)
                .map_err(|error| error.into_error(process_name, process_path))?;

            if !process.status.success() {
                return Err(Error::ConversionFailed {
                    reason: describe_process_failure(process_name, &process),
                });
            }

            validate_created_output(temporary_output.as_ref())
        })();

        let output_metadata = match conversion {
            Ok(metadata) => metadata,
            Err(error) => {
                return Err(cleanup_owned_temporary(error, temporary_output.as_ref()));
            }
        };

        publish_without_replacing(temporary_output, &request.output)?;

        Ok(TextureTranscodeReport {
            output: request.output.clone(),
            input_bytes,
            output_bytes: output_metadata.len(),
            compression: request.compression,
            reduction: request.reduction,
        })
    }
}

fn append_compiler_arguments(
    command: &mut Command,
    input: &OsStr,
    output: &OsStr,
    request: &TextureTranscodeRequest,
) {
    command
        .arg("-transcode")
        .arg("-i")
        .arg(input)
        .arg("-o")
        .arg(output)
        .arg("-f")
        .arg("ETC2");

    if request.compression == Compression::HighPerformance {
        command.arg("-c").arg("force");
    }

    command
        .arg("-shrink")
        .arg(match request.reduction {
            Reduction::Original => "1",
            Reduction::X2 => "2",
            Reduction::X4 => "4",
        })
        .arg("-maxmipmaps")
        .arg("1");
}

fn validate_backend_file(path: &Path, name: &str) -> Result<()> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(Error::BackendUnavailable {
                backend: format!("{name} not found at {}", path.display()),
            });
        }
        Err(error) => {
            return Err(Error::BackendUnavailable {
                backend: format!("cannot access {name} at {}: {error}", path.display()),
            });
        }
    };

    if !metadata.is_file() {
        return Err(Error::ConversionFailed {
            reason: format!("{name} is not a regular file: {}", path.display()),
        });
    }
    Ok(())
}

fn validate_input_file(path: &Path) -> Result<Metadata> {
    let metadata = fs::metadata(path).map_err(|error| Error::ConversionFailed {
        reason: format!("cannot access texture input {}: {error}", path.display()),
    })?;
    if !metadata.is_file() {
        return Err(Error::ConversionFailed {
            reason: format!("texture input is not a regular file: {}", path.display()),
        });
    }
    Ok(metadata)
}

fn validate_output_is_absent(input: &Path, output: &Path) -> Result<()> {
    match fs::symlink_metadata(output) {
        Ok(_) => {
            if fs::metadata(output).is_ok() && paths_refer_to_same_file(input, output)? {
                return Err(Error::ConversionFailed {
                    reason: format!(
                        "texture input and output resolve to the same file: {}",
                        input.display()
                    ),
                });
            }
            Err(Error::ConversionFailed {
                reason: format!("texture output already exists: {}", output.display()),
            })
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::ConversionFailed {
            reason: format!(
                "cannot inspect texture output {}: {error}",
                output.display()
            ),
        }),
    }
}

fn paths_refer_to_same_file(left: &Path, right: &Path) -> Result<bool> {
    same_file::is_same_file(left, right).map_err(|error| Error::ConversionFailed {
        reason: format!(
            "cannot compare texture input {} and output {}: {error}",
            left.display(),
            right.display()
        ),
    })
}

fn validate_created_output(path: &Path) -> Result<Metadata> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(Error::ConversionFailed {
                reason: format!(
                    "resource compiler did not create texture output {}",
                    path.display()
                ),
            });
        }
        Err(error) => {
            return Err(Error::ConversionFailed {
                reason: format!("cannot inspect texture output {}: {error}", path.display()),
            });
        }
    };

    if !metadata.is_file() {
        return Err(Error::ConversionFailed {
            reason: format!(
                "resource compiler output is not a regular file: {}",
                path.display()
            ),
        });
    }
    if metadata.len() == 0 {
        return Err(Error::ConversionFailed {
            reason: format!(
                "resource compiler created an empty output: {}",
                path.display()
            ),
        });
    }

    let mut header = [0_u8; TEX_V5_MAGIC.len()];
    let header_result = File::open(path).and_then(|mut file| file.read_exact(&mut header));
    if header_result.is_err() || &header != TEX_V5_MAGIC {
        return Err(Error::ConversionFailed {
            reason: format!(
                "resource compiler output does not begin with TEXV0005\\0: {}",
                path.display()
            ),
        });
    }

    Ok(metadata)
}

fn create_owned_temporary_output(output: &Path) -> Result<TempPath> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let temporary = tempfile::Builder::new()
        .prefix(".pkg2mpkg-resource-")
        .suffix(".tex")
        .tempfile_in(parent)
        .map_err(|error| Error::ConversionFailed {
            reason: format!(
                "cannot reserve temporary texture output beside {}: {error}",
                output.display()
            ),
        })?
        .into_temp_path();

    fs::remove_file(&temporary).map_err(|error| Error::ConversionFailed {
        reason: format!(
            "cannot prepare temporary texture output {}: {error}",
            temporary.display()
        ),
    })?;

    Ok(temporary)
}

fn publish_without_replacing(temporary: TempPath, output: &Path) -> Result<()> {
    publish_without_replacing_with(temporary, output, atomic_rename_noreplace)
}

fn publish_without_replacing_with(
    mut temporary: TempPath,
    output: &Path,
    atomic_publish: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> Result<()> {
    match atomic_publish(temporary.as_ref(), output) {
        Ok(()) => {
            // The atomic publisher consumed the path. Disarm TempPath without
            // touching the filesystem: another actor may already have created
            // a new file at the now-vacant staging name.
            temporary.disable_cleanup(true);
            Ok(())
        }
        Err(source) => {
            let reason = if source.kind() == io::ErrorKind::AlreadyExists {
                format!("texture output already exists: {}", output.display())
            } else {
                format!(
                    "cannot atomically publish texture output {} without replacing it: {source}",
                    output.display()
                )
            };
            Err(cleanup_owned_temporary(
                Error::ConversionFailed { reason },
                temporary.as_ref(),
            ))
        }
    }
}

#[cfg(any(
    target_os = "android",
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "visionos",
    target_os = "watchos",
    target_os = "redox",
))]
fn atomic_rename_noreplace(source: &Path, output: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    renameat_with(CWD, source, CWD, output, RenameFlags::NOREPLACE).map_err(Into::into)
}

#[cfg(windows)]
fn atomic_rename_noreplace(source: &Path, output: &Path) -> io::Result<()> {
    // atomicwrites::move_atomic uses MoveFileExW with MOVEFILE_WRITE_THROUGH
    // and deliberately omits MOVEFILE_REPLACE_EXISTING.
    atomicwrites::move_atomic(source, output)
}

#[cfg(not(any(
    windows,
    target_os = "android",
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "visionos",
    target_os = "watchos",
    target_os = "redox",
)))]
fn atomic_rename_noreplace(_source: &Path, _output: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-replace publication is unavailable on this platform",
    ))
}

fn cleanup_owned_temporary(error: Error, output: &Path) -> Error {
    let cleanup_error = match fs::symlink_metadata(output) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => Some(format!(
            "could not inspect temporary texture output {} for cleanup: {error}",
            output.display()
        )),
        Ok(metadata) if metadata.file_type().is_dir() => {
            fs::remove_dir(output).err().map(|error| {
                format!(
                    "could not remove directory at temporary texture output {}: {error}",
                    output.display()
                )
            })
        }
        Ok(_) => fs::remove_file(output).err().map(|error| {
            format!(
                "could not remove temporary texture output {}: {error}",
                output.display()
            )
        }),
    };

    match (error, cleanup_error) {
        (Error::ConversionFailed { mut reason }, Some(cleanup_error)) => {
            reason.push_str("; ");
            reason.push_str(&cleanup_error);
            Error::ConversionFailed { reason }
        }
        (error, _) => error,
    }
}

pub(crate) struct CapturedProcess {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: CapturedStream,
    pub(crate) stderr: CapturedStream,
}

pub(crate) struct CapturedStream {
    bytes: Vec<u8>,
    truncated: bool,
}

impl CapturedStream {
    #[cfg(not(windows))]
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[cfg(not(windows))]
    pub(crate) const fn truncated(&self) -> bool {
        self.truncated
    }

    fn diagnostic(&self) -> String {
        let text = String::from_utf8_lossy(&self.bytes);
        let text = text.trim();
        let mut diagnostic = if text.is_empty() {
            "<empty>".to_owned()
        } else {
            text.to_owned()
        };
        if self.truncated {
            diagnostic.push_str(" [truncated]");
        }
        diagnostic
    }
}

pub(crate) struct ProcessRunError {
    kind: ProcessRunErrorKind,
}

enum ProcessRunErrorKind {
    Spawn(io::Error),
    Wait(io::Error),
    Capture(&'static str, io::Error),
    CapturePanicked(&'static str),
}

impl ProcessRunError {
    pub(crate) fn into_error(self, name: &str, path: &Path) -> Error {
        match self.kind {
            ProcessRunErrorKind::Spawn(error) if error.kind() == io::ErrorKind::NotFound => {
                Error::BackendUnavailable {
                    backend: format!(
                        "{name} could not be launched at {}: {error}",
                        path.display()
                    ),
                }
            }
            ProcessRunErrorKind::Spawn(error) => Error::ConversionFailed {
                reason: format!("could not launch {name} at {}: {error}", path.display()),
            },
            ProcessRunErrorKind::Wait(error) => Error::ConversionFailed {
                reason: format!(
                    "failed while waiting for {name} at {}: {error}",
                    path.display()
                ),
            },
            ProcessRunErrorKind::Capture(stream, error) => Error::ConversionFailed {
                reason: format!(
                    "failed while reading {name} {stream} at {}: {error}",
                    path.display()
                ),
            },
            ProcessRunErrorKind::CapturePanicked(stream) => Error::ConversionFailed {
                reason: format!("failed while reading {name} {stream} at {}", path.display()),
            },
        }
    }
}

pub(crate) fn run_bounded(
    mut command: Command,
) -> std::result::Result<CapturedProcess, ProcessRunError> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| ProcessRunError {
        kind: ProcessRunErrorKind::Spawn(error),
    })?;

    let stdout = child.stdout.take().expect("piped stdout must be available");
    let stderr = child.stderr.take().expect("piped stderr must be available");
    let stdout_reader = thread::spawn(move || read_bounded(stdout));
    let stderr_reader = thread::spawn(move || read_bounded(stderr));

    let status = child.wait();
    let stdout = join_capture(stdout_reader, "stdout");
    let stderr = join_capture(stderr_reader, "stderr");

    let status = status.map_err(|error| ProcessRunError {
        kind: ProcessRunErrorKind::Wait(error),
    })?;
    let stdout = stdout?;
    let stderr = stderr?;

    Ok(CapturedProcess {
        status,
        stdout,
        stderr,
    })
}

fn read_bounded(mut reader: impl Read) -> io::Result<CapturedStream> {
    let mut bytes = Vec::with_capacity(DIAGNOSTIC_LIMIT);
    let mut buffer = [0_u8; 4096];
    let mut truncated = false;

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = DIAGNOSTIC_LIMIT.saturating_sub(bytes.len());
        let retained = read.min(remaining);
        bytes.extend_from_slice(&buffer[..retained]);
        truncated |= retained < read;
    }

    Ok(CapturedStream { bytes, truncated })
}

fn join_capture(
    reader: thread::JoinHandle<io::Result<CapturedStream>>,
    stream: &'static str,
) -> std::result::Result<CapturedStream, ProcessRunError> {
    match reader.join() {
        Ok(Ok(captured)) => Ok(captured),
        Ok(Err(error)) => Err(ProcessRunError {
            kind: ProcessRunErrorKind::Capture(stream, error),
        }),
        Err(_) => Err(ProcessRunError {
            kind: ProcessRunErrorKind::CapturePanicked(stream),
        }),
    }
}

pub(crate) fn describe_process_failure(name: &str, process: &CapturedProcess) -> String {
    format!(
        "{name} exited with {}; stdout: {}; stderr: {}",
        process.status,
        process.stdout.diagnostic(),
        process.stderr.diagnostic()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn unsupported_atomic_publish_fails_closed_and_preserves_the_existing_output() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("final.tex");
        fs::write(&output, b"incumbent").unwrap();

        let mut staged = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        staged.write_all(b"candidate").unwrap();
        let staged = staged.into_temp_path();
        let staged_path = staged.to_path_buf();

        let error = publish_without_replacing_with(staged, &output, |_source, _destination| {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "atomic no-replace is unavailable",
            ))
        })
        .unwrap_err();

        assert_eq!(error.code(), pkg2mpkg_core::ErrorCode::ConversionFailed);
        assert!(
            error
                .to_string()
                .contains("atomic no-replace is unavailable")
        );
        assert_eq!(fs::read(&output).unwrap(), b"incumbent");
        assert!(!staged_path.exists(), "owned staged output leaked");
    }

    #[test]
    fn successful_atomic_publish_does_not_delete_recreated_staging_path() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("final.tex");

        let mut staged = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        staged.write_all(b"candidate").unwrap();
        let staged = staged.into_temp_path();
        let staged_path = staged.to_path_buf();

        publish_without_replacing_with(staged, &output, |source, destination| {
            fs::rename(source, destination)?;
            fs::write(source, b"competitor")?;
            Ok(())
        })
        .unwrap();

        assert_eq!(fs::read(&output).unwrap(), b"candidate");
        assert_eq!(
            fs::read(&staged_path).unwrap(),
            b"competitor",
            "TempPath drop removed a concurrently recreated staging path"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_atomic_publish_collision_preserves_incumbent_and_cleans_owned_stage() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("final.tex");

        let mut staged = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        staged.write_all(b"candidate").unwrap();
        let staged = staged.into_temp_path();
        let staged_path = staged.to_path_buf();

        // Model the real race: validation and staging succeeded before another
        // caller published the destination.
        fs::write(&output, b"incumbent").unwrap();

        let error = publish_without_replacing(staged, &output).unwrap_err();

        assert_eq!(error.code(), pkg2mpkg_core::ErrorCode::ConversionFailed);
        assert_eq!(fs::read(&output).unwrap(), b"incumbent");
        assert!(!staged_path.exists(), "owned staged output leaked");
    }
}
