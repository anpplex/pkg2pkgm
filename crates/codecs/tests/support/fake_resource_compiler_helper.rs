use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process::{self, Command},
    thread,
    time::{Duration, Instant},
};

const TEX_V5: &[u8] = b"TEXV0005\0converted";
const WAIT_TIMEOUT: Duration = Duration::from_secs(60);
const ARGV_MAGIC: &[u8] = b"ARGV0001";

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

fn main() {
    if let Err(error) = run() {
        eprintln!("fake resource compiler failed: {error}");
        process::exit(70);
    }
}

fn run() -> io::Result<()> {
    let executable = env::current_exe()?;
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    append_invocation(&sidecar(&executable, ".argv"), &arguments)?;

    let control = fs::read_to_string(sidecar(&executable, ".control"))?;
    let mut lines = control.lines();
    let behavior = lines.next().unwrap_or_default();

    match behavior {
        "success" => write_output(&arguments, TEX_V5),
        "nonzero" => process::exit(23),
        "no-output" => Ok(()),
        "empty" => write_output(&arguments, b""),
        "bad-magic" => write_output(&arguments, b"not-a-texture"),
        "large-stderr" => {
            io::stderr().write_all(&vec![b'x'; 20_000])?;
            process::exit(9);
        }
        "wait-success" => {
            let started = required_path(&mut lines, "started path")?;
            let release = required_path(&mut lines, "release path")?;
            fs::write(started, b"started")?;
            wait_for(&release)?;
            write_output(&arguments, TEX_V5)
        }
        "signal-wait-fail" => {
            let signal = required_path(&mut lines, "signal path")?;
            let release = required_path(&mut lines, "release path")?;
            fs::write(signal, b"started")?;
            wait_for(&release)?;
            process::exit(24);
        }
        "signal-wait-success" => {
            let signal = required_path(&mut lines, "signal path")?;
            let release = required_path(&mut lines, "release path")?;
            fs::write(signal, b"started")?;
            wait_for(&release)?;
            write_output(&arguments, TEX_V5)
        }
        "winepath" => emulate_winepath(&arguments),
        "wine-forward" => emulate_wine(&arguments),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unknown behavior {other:?}"),
        )),
    }
}

fn emulate_wine(arguments: &[OsString]) -> io::Result<()> {
    let (compiler, arguments) = arguments
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing compiler argv[0]"))?;
    let status = Command::new(compiler).args(arguments).status()?;
    match status.code() {
        Some(code) => process::exit(code),
        None => process::exit(71),
    }
}

fn required_path<'a>(
    lines: &mut impl Iterator<Item = &'a str>,
    label: &str,
) -> io::Result<PathBuf> {
    lines.next().map(PathBuf::from).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("missing {label} in control file"),
        )
    })
}

fn write_output(arguments: &[OsString], contents: &[u8]) -> io::Result<()> {
    let output = value_after(arguments, OsStr::new("-o"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing -o output argument"))?;
    fs::write(output, contents)
}

fn emulate_winepath(arguments: &[OsString]) -> io::Result<()> {
    if arguments.len() != 2 || arguments[0] != OsStr::new("-w") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "winepath expects exactly '-w PATH'",
        ));
    }
    let path = arguments[1].to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "fake winepath only supports Unicode paths",
        )
    })?;
    io::stdout().write_all(path.as_bytes())?;
    io::stdout().write_all(b"\n")
}

fn value_after<'a>(arguments: &'a [OsString], flag: &OsStr) -> Option<&'a OsStr> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].as_os_str())
}

fn wait_for(path: &Path) -> io::Result<()> {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    while !path.exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("timed out waiting for {}", path.display()),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

fn append_invocation(path: &Path, arguments: &[OsString]) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    if file.metadata()?.len() == 0 {
        file.write_all(ARGV_MAGIC)?;
        #[cfg(unix)]
        file.write_all(b"U")?;
        #[cfg(windows)]
        file.write_all(b"W")?;
    }
    write_u32(&mut file, arguments.len())?;
    for argument in arguments {
        #[cfg(unix)]
        {
            let argument = argument.as_os_str().as_bytes();
            write_u32(&mut file, argument.len())?;
            file.write_all(argument)?;
        }
        #[cfg(windows)]
        {
            let argument = argument.as_os_str().encode_wide().collect::<Vec<_>>();
            write_u32(&mut file, argument.len())?;
            for unit in argument {
                file.write_all(&unit.to_le_bytes())?;
            }
        }
    }
    Ok(())
}

fn write_u32(writer: &mut impl Write, value: usize) -> io::Result<()> {
    let value = u32::try_from(value)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "value exceeds u32"))?;
    writer.write_all(&value.to_le_bytes())
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut path = path.as_os_str().to_owned();
    path.push(suffix);
    PathBuf::from(path)
}
