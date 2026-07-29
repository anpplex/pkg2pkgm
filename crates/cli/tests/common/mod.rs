//! Shared helpers for CLI integration tests (fake WE runtime + Wine helpers).
#![allow(dead_code)]

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use assert_cmd::Command;
use tempfile::TempDir;

const COMPILED_HELPER: &str = env!("CARGO_BIN_EXE_pkg2mpkg-cli-fake-resource-compiler");
const ARGV_MAGIC: &[u8] = b"ARGV0001";

pub fn pkg2mpkg() -> Command {
    Command::cargo_bin("pkg2mpkg").unwrap()
}

pub fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut path = path.as_os_str().to_owned();
    path.push(suffix);
    PathBuf::from(path)
}

/// Install the Rust-compiled fake helper under `root` with a control file.
pub fn install_helper(root: &Path, name: &str, behavior: &str) -> PathBuf {
    let helper = root.join(format!("{name}{}", env::consts::EXE_SUFFIX));
    fs::copy(COMPILED_HELPER, &helper).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&helper).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&helper, perms).unwrap();
    }
    let mut control = behavior.to_owned();
    control.push('\n');
    fs::write(sidecar(&helper, ".control"), control).unwrap();
    helper
}

/// Layout: `<runtime>/distribution/bin/resourcecompiler64.exe` (+ optional Wine pair).
pub struct FakeRuntime {
    pub dir: TempDir,
    pub runtime: PathBuf,
    pub compiler: PathBuf,
    pub wine: Option<PathBuf>,
    pub winepath: Option<PathBuf>,
}

impl FakeRuntime {
    /// Successful fake compiler under a WE-like runtime tree.
    pub fn with_success_compiler() -> Self {
        Self::with_compiler_behavior("success")
    }

    pub fn with_compiler_behavior(behavior: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("Wallpaper Engine.2.8.26");
        let bin = runtime.join("distribution/bin");
        fs::create_dir_all(&bin).unwrap();
        // PE name is fixed; on Unix it need not be +x. Under Wine mode the
        // host process is the fake Wine binary (not the PE), so conversion
        // behavior is attached there on non-Windows hosts.
        let compiler = bin.join("resourcecompiler64.exe");

        #[cfg(windows)]
        {
            fs::copy(COMPILED_HELPER, &compiler).unwrap();
            let mut control = behavior.to_owned();
            control.push('\n');
            fs::write(sidecar(&compiler, ".control"), control).unwrap();
            FakeRuntime {
                dir,
                runtime,
                compiler,
                wine: None,
                winepath: None,
            }
        }

        #[cfg(not(windows))]
        {
            // PE is only path-resolved; content is irrelevant under fake Wine.
            fs::write(&compiler, b"MZ-fake-resourcecompiler64").unwrap();
            let tools = dir.path().join("tools");
            fs::create_dir_all(&tools).unwrap();
            let wine = install_helper(&tools, "wine", behavior);
            let winepath = install_helper(&tools, "winepath", "winepath");
            FakeRuntime {
                dir,
                runtime,
                compiler,
                wine: Some(wine),
                winepath: Some(winepath),
            }
        }
    }

    /// Append platform Wine flags when the host is not Windows.
    pub fn apply_launch_flags(&self, cmd: &mut Command) {
        cmd.arg("--we-runtime").arg(&self.runtime);
        #[cfg(not(windows))]
        {
            cmd.arg("--wine").arg(self.wine.as_ref().unwrap());
            // Explicit winepath keeps tests independent of PATH layout.
            cmd.arg("--winepath").arg(self.winepath.as_ref().unwrap());
        }
    }

    pub fn compiler_argv_path(&self) -> PathBuf {
        #[cfg(windows)]
        {
            sidecar(&self.compiler, ".argv")
        }
        #[cfg(not(windows))]
        {
            // Wine mode: the wine binary is the process that records argv.
            sidecar(self.wine.as_ref().unwrap(), ".argv")
        }
    }

    pub fn wine_argv_path(&self) -> Option<PathBuf> {
        self.wine.as_ref().map(|wine| sidecar(wine, ".argv"))
    }

    pub fn winepath_argv_path(&self) -> Option<PathBuf> {
        self.winepath
            .as_ref()
            .map(|winepath| sidecar(winepath, ".argv"))
    }

    pub fn any_helper_started(&self) -> bool {
        if sidecar(&self.compiler, ".argv").exists() {
            return true;
        }
        if self.wine_argv_path().is_some_and(|path| path.exists()) {
            return true;
        }
        self.winepath_argv_path().is_some_and(|path| path.exists())
    }
}

pub fn read_invocations(argv_path: &Path) -> Vec<Vec<String>> {
    let bytes = match fs::read(argv_path) {
        Ok(bytes) => bytes,
        Err(_) => return Vec::new(),
    };
    assert!(bytes.starts_with(ARGV_MAGIC), "missing raw argv header");
    let mut cursor = ARGV_MAGIC.len();
    let encoding = *bytes.get(cursor).expect("missing raw argv encoding");
    cursor += 1;
    #[cfg(unix)]
    assert_eq!(encoding, b'U');
    #[cfg(windows)]
    assert_eq!(encoding, b'W');
    let mut invocations = Vec::new();
    while cursor < bytes.len() {
        let argument_count = read_u32(&bytes, &mut cursor);
        let mut arguments = Vec::with_capacity(argument_count);
        for _ in 0..argument_count {
            let units = read_u32(&bytes, &mut cursor);
            #[cfg(unix)]
            let (argument, end) = {
                let end = cursor.checked_add(units).expect("argv length overflow");
                let raw = bytes.get(cursor..end).expect("truncated argv record");
                (
                    String::from_utf8(raw.to_vec()).expect("argv must be UTF-8"),
                    end,
                )
            };
            #[cfg(windows)]
            let (argument, end) = {
                let byte_length = units.checked_mul(2).expect("argv length overflow");
                let end = cursor
                    .checked_add(byte_length)
                    .expect("argv length overflow");
                let raw = bytes.get(cursor..end).expect("truncated argv record");
                let wide = raw
                    .chunks_exact(2)
                    .map(|unit| u16::from_le_bytes(unit.try_into().unwrap()))
                    .collect::<Vec<_>>();
                (
                    String::from_utf16(&wide).expect("argv must be valid Unicode"),
                    end,
                )
            };
            arguments.push(argument);
            cursor = end;
        }
        invocations.push(arguments);
    }
    invocations
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> usize {
    let end = cursor.checked_add(4).expect("argv cursor overflow");
    let value = bytes.get(*cursor..end).expect("truncated argv header");
    *cursor = end;
    u32::from_le_bytes(value.try_into().unwrap()) as usize
}

/// Assert helper argv contains expected shrink/compression flags for one texture.
pub fn assert_compiler_flags(arguments: &[String], force: bool, shrink: &str) {
    assert!(
        arguments.iter().any(|arg| arg == "-transcode"),
        "missing -transcode in {arguments:?}"
    );
    assert!(
        arguments.iter().any(|arg| arg == "ETC2"),
        "missing ETC2 in {arguments:?}"
    );
    let shrink_pos = arguments
        .iter()
        .position(|arg| arg == "-shrink")
        .expect("missing -shrink");
    assert_eq!(
        arguments.get(shrink_pos + 1).map(String::as_str),
        Some(shrink)
    );
    let has_force = arguments.windows(2).any(|pair| pair == ["-c", "force"]);
    assert_eq!(has_force, force, "force flag mismatch in {arguments:?}");
}
