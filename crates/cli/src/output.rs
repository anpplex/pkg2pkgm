use std::{io, io::Write, path::PathBuf};

use pkg2mpkg_core::{Error, ErrorCode, Result, Stage};
use serde::Serialize;

#[derive(Serialize)]
struct ErrorOutput<'a> {
    code: ErrorCode,
    stage: Stage,
    message: &'a str,
}

pub fn print_json(value: &impl Serialize) -> Result<()> {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    serde_json::to_writer_pretty(&mut lock, value)
        .map_err(|source| output_error(io::Error::other(source)))?;
    writeln!(lock).map_err(output_error)
}

pub fn print_text(value: &str) -> Result<()> {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    writeln!(lock, "{value}").map_err(output_error)
}

pub fn print_error(error: &Error, json: bool) {
    let stderr = io::stderr();
    let mut lock = stderr.lock();
    if json {
        let message = error.to_string();
        let value = ErrorOutput {
            code: error.code(),
            stage: error.stage(),
            message: &message,
        };
        if serde_json::to_writer_pretty(&mut lock, &value).is_ok() {
            let _ = writeln!(lock);
        }
    } else {
        let _ = writeln!(lock, "error: {error}");
    }
}

fn output_error(source: io::Error) -> Error {
    Error::Io {
        stage: Stage::Plan,
        path: PathBuf::from("<stdout>"),
        source,
    }
}
