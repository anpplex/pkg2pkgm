use std::{
    collections::BTreeSet,
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use pkg2mpkg_codecs::ResourceCompilerBackend;
use pkg2mpkg_core::{
    Compression, ErrorCode, Reduction, ResourceTranscodeBackend, TextureTranscodeRequest,
};
use tempfile::tempdir;

#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStringExt;

const COMPILED_HELPER: &str = env!("CARGO_BIN_EXE_pkg2mpkg-fake-resource-compiler");
// CI runners can be under heavy parallel test load; give the fake helper more
// time to spawn and write its started-signal file before the parent asserts.
const WAIT_TIMEOUT: Duration = Duration::from_secs(60);

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut path = path.as_os_str().to_owned();
    path.push(suffix);
    PathBuf::from(path)
}

fn install_helper(root: &Path, name: &str, behavior: &str, paths: &[&Path]) -> PathBuf {
    let helper = root.join(format!("{name}{}", env::consts::EXE_SUFFIX));
    fs::copy(COMPILED_HELPER, &helper).unwrap();

    let mut control = behavior.to_owned();
    for path in paths {
        let path = path.to_str().expect("test paths must be valid Unicode");
        assert!(!path.contains(['\n', '\r']));
        control.push('\n');
        control.push_str(path);
    }
    control.push('\n');
    fs::write(sidecar(&helper, ".control"), control).unwrap();
    helper
}

fn request(root: &Path, compression: Compression, reduction: Reduction) -> TextureTranscodeRequest {
    let input = root.join("源 texture with spaces.tex");
    fs::write(&input, b"desktop texture").unwrap();
    TextureTranscodeRequest::new(
        input,
        root.join("输出 texture with spaces.tex"),
        compression,
        reduction,
    )
    .unwrap()
}

fn invocations(helper: &Path) -> Vec<Vec<String>> {
    invocations_os(helper)
        .into_iter()
        .map(|arguments| {
            arguments
                .into_iter()
                .map(|argument| argument.into_string().expect("argv must be valid Unicode"))
                .collect()
        })
        .collect()
}

fn invocations_os(helper: &Path) -> Vec<Vec<OsString>> {
    let bytes = fs::read(sidecar(helper, ".argv")).unwrap();
    assert!(bytes.starts_with(b"ARGV0001"), "missing raw argv header");
    let mut cursor = b"ARGV0001".len();
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
                let argument = bytes.get(cursor..end).expect("truncated argv record");
                (OsString::from_vec(argument.to_vec()), end)
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
                (OsString::from_wide(&wide), end)
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

fn only_invocation(helper: &Path) -> Vec<String> {
    let calls = invocations(helper);
    assert_eq!(calls.len(), 1, "expected one helper invocation: {calls:?}");
    calls.into_iter().next().unwrap()
}

fn value_after(arguments: &[String], flag: &str) -> PathBuf {
    arguments
        .windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| PathBuf::from(&pair[1]))
        .unwrap_or_else(|| panic!("missing {flag} in {arguments:?}"))
}

fn value_after_os(arguments: &[OsString], flag: &OsStr) -> PathBuf {
    arguments
        .windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| PathBuf::from(&pair[1]))
        .unwrap_or_else(|| panic!("missing {flag:?} in {arguments:?}"))
}

fn assert_owned_temp_output(arguments: &[String], final_output: &Path) -> PathBuf {
    let temporary_output = value_after(arguments, "-o");
    assert_ne!(temporary_output, final_output);
    assert_eq!(temporary_output.parent(), final_output.parent());
    assert_eq!(temporary_output.extension(), Some(OsStr::new("tex")));
    assert!(
        !temporary_output.exists(),
        "temporary output leaked at {}",
        temporary_output.display()
    );
    temporary_output
}

fn directory_entries(root: &Path) -> BTreeSet<OsString> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect()
}

fn wait_for_path(path: &Path) {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn host_backend(compiler: &Path) -> ResourceCompilerBackend {
    #[cfg(windows)]
    {
        ResourceCompilerBackend::native(compiler)
    }

    #[cfg(not(windows))]
    {
        let root = compiler.parent().unwrap();
        let tag = compiler
            .file_name()
            .and_then(OsStr::to_str)
            .expect("test helper name must be valid Unicode");
        let wine = install_helper(root, &format!("{tag}-wine"), "wine-forward", &[]);
        let winepath = install_helper(root, &format!("{tag}-winepath"), "winepath", &[]);
        ResourceCompilerBackend::wine(compiler, wine, winepath)
    }
}

#[test]
fn helper_receives_exact_input_and_owned_output_paths_without_shell_interpretation() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("parent [];$&'()# 中文 🚗");
    fs::create_dir(&parent).unwrap();
    let helper = install_helper(&parent, "compiler", "success", &[]);
    let mut request = request(&parent, Compression::HighQuality, Reduction::Original);
    let special_input = parent.join("input $HOME;$(touch nope)&' [].jpg");
    fs::rename(&request.input, &special_input).unwrap();
    request.input = special_input;
    request.output = parent.join("final output;$(touch nope)&' [].tex");

    host_backend(&helper).transcode_texture(&request).unwrap();

    let calls = invocations_os(&helper);
    assert_eq!(calls.len(), 1, "expected one helper invocation: {calls:?}");
    let received_input = value_after_os(&calls[0], OsStr::new("-i"));
    let temporary_output = value_after_os(&calls[0], OsStr::new("-o"));
    assert_eq!(received_input, request.input);
    assert_eq!(temporary_output.parent(), request.output.parent());
    assert_ne!(temporary_output, request.output);
    assert_eq!(temporary_output.extension(), Some(OsStr::new("tex")));
    assert!(!temporary_output.exists());
    assert_eq!(fs::read(&request.output).unwrap(), b"TEXV0005\0converted");
    assert!(
        !parent.join("nope").exists(),
        "path text was shell-expanded"
    );
}

#[test]
fn high_quality_original_matches_the_windows_2826_arguments() {
    let dir = tempdir().unwrap();
    let helper = install_helper(dir.path(), "fake compiler", "success", &[]);
    let request = request(dir.path(), Compression::HighQuality, Reduction::Original);

    let report = host_backend(&helper).transcode_texture(&request).unwrap();

    let arguments = only_invocation(&helper);
    let temporary_output = assert_owned_temp_output(&arguments, &request.output);
    assert_eq!(
        arguments,
        vec![
            "-transcode",
            "-i",
            request.input.to_str().unwrap(),
            "-o",
            temporary_output.to_str().unwrap(),
            "-f",
            "ETC2",
            "-shrink",
            "1",
            "-maxmipmaps",
            "1",
        ]
    );
    assert_eq!(report.input_bytes, 15);
    assert!(report.output_bytes > 9);
    assert!(request.output.exists());
}

#[test]
fn high_performance_adds_force_and_maps_reduction_factors() {
    for (reduction, factor) in [(Reduction::X2, "2"), (Reduction::X4, "4")] {
        let dir = tempdir().unwrap();
        let helper = install_helper(dir.path(), "compiler", "success", &[]);
        let request = request(dir.path(), Compression::HighPerformance, reduction);

        host_backend(&helper).transcode_texture(&request).unwrap();

        let arguments = only_invocation(&helper);
        assert_owned_temp_output(&arguments, &request.output);
        assert_eq!(
            &arguments[7..],
            &["-c", "force", "-shrink", factor, "-maxmipmaps", "1"]
        );
    }
}

#[test]
fn helper_failures_and_suspicious_outputs_are_conversion_errors() {
    let cases = [
        ("nonzero", "exited"),
        ("no-output", "did not create"),
        ("empty", "empty"),
        ("bad-magic", "TEXV0005"),
    ];
    for (index, (behavior, message)) in cases.into_iter().enumerate() {
        let dir = tempdir().unwrap();
        let helper = install_helper(dir.path(), &format!("compiler-{index}"), behavior, &[]);
        let request = request(dir.path(), Compression::HighQuality, Reduction::Original);

        let error = host_backend(&helper)
            .transcode_texture(&request)
            .unwrap_err();

        assert_eq!(error.code(), ErrorCode::ConversionFailed, "case {index}");
        assert!(error.to_string().contains(message), "case {index}: {error}");
        assert!(!request.output.exists(), "case {index}");
        let arguments = only_invocation(&helper);
        assert_owned_temp_output(&arguments, &request.output);
    }
}

#[test]
fn stale_output_is_rejected_without_launching_or_changing_the_directory() {
    let dir = tempdir().unwrap();
    let helper = install_helper(dir.path(), "compiler", "success", &[]);
    let request = request(dir.path(), Compression::HighQuality, Reduction::Original);
    fs::write(&request.output, b"preserve me").unwrap();
    let backend = host_backend(&helper);
    let before = directory_entries(dir.path());

    let error = backend.transcode_texture(&request).unwrap_err();

    assert_eq!(error.code(), ErrorCode::ConversionFailed);
    assert!(error.to_string().contains("already exists"));
    assert_eq!(fs::read(&request.output).unwrap(), b"preserve me");
    assert_eq!(directory_entries(dir.path()), before);
    assert!(!sidecar(&helper, ".argv").exists());
}

#[test]
fn input_and_output_hard_links_are_rejected_before_launch() {
    let dir = tempdir().unwrap();
    let helper = install_helper(dir.path(), "compiler", "success", &[]);
    let request = request(dir.path(), Compression::HighQuality, Reduction::Original);
    fs::hard_link(&request.input, &request.output).unwrap();

    let error = host_backend(&helper)
        .transcode_texture(&request)
        .unwrap_err();

    assert_eq!(error.code(), ErrorCode::ConversionFailed);
    assert!(error.to_string().contains("same file"));
    assert_eq!(fs::read(&request.input).unwrap(), b"desktop texture");
    assert!(!sidecar(&helper, ".argv").exists());
}

#[test]
fn max_mipmaps_other_than_one_is_rejected_before_launch() {
    let dir = tempdir().unwrap();
    let helper = install_helper(dir.path(), "compiler", "success", &[]);
    let mut request = request(dir.path(), Compression::HighQuality, Reduction::Original);
    request.max_mipmaps = 2;

    let error = host_backend(&helper)
        .transcode_texture(&request)
        .unwrap_err();

    assert_eq!(error.code(), ErrorCode::InvalidArguments);
    assert!(error.to_string().contains("max_mipmaps 1"));
    assert!(!sidecar(&helper, ".argv").exists());
}

#[test]
fn missing_compiler_and_wine_are_backend_unavailable() {
    let dir = tempdir().unwrap();
    let request = request(dir.path(), Compression::HighQuality, Reduction::Original);
    let error = host_backend(&dir.path().join("missing compiler"))
        .transcode_texture(&request)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::BackendUnavailable);

    #[cfg(not(windows))]
    {
        let compiler = dir.path().join("compiler.exe");
        fs::write(&compiler, b"supplied compiler").unwrap();
        let winepath = install_helper(dir.path(), "winepath", "winepath", &[]);
        let error =
            ResourceCompilerBackend::wine(compiler, dir.path().join("missing wine"), &winepath)
                .transcode_texture(&request)
                .unwrap_err();
        assert_eq!(error.code(), ErrorCode::BackendUnavailable);
        assert!(!sidecar(&winepath, ".argv").exists());
    }
}

#[cfg(not(windows))]
#[test]
fn wine_mode_translates_the_owned_temporary_output_then_invokes_wine() {
    let dir = tempdir().unwrap();
    let compiler = dir.path().join("resource compiler 非ASCII.exe");
    fs::write(&compiler, b"supplied compiler").unwrap();
    let winepath = install_helper(dir.path(), "winepath", "winepath", &[]);
    let wine = install_helper(dir.path(), "wine", "success", &[]);
    let request = request(dir.path(), Compression::HighPerformance, Reduction::X2);

    ResourceCompilerBackend::wine(&compiler, &wine, &winepath)
        .transcode_texture(&request)
        .unwrap();

    let winepath_calls = invocations(&winepath);
    assert_eq!(winepath_calls.len(), 3);
    assert_eq!(winepath_calls[0], vec!["-w", compiler.to_str().unwrap()]);
    assert_eq!(
        winepath_calls[1],
        vec!["-w", request.input.to_str().unwrap()]
    );

    let temporary_output = PathBuf::from(&winepath_calls[2][1]);
    assert_ne!(temporary_output, request.output);
    assert_eq!(
        winepath_calls[2],
        vec!["-w", temporary_output.to_str().unwrap()]
    );

    let arguments = only_invocation(&wine);
    assert_eq!(value_after(&arguments, "-o"), temporary_output);
    assert_owned_temp_output(&arguments, &request.output);
    assert_eq!(
        arguments,
        vec![
            compiler.to_str().unwrap(),
            "-transcode",
            "-i",
            request.input.to_str().unwrap(),
            "-o",
            temporary_output.to_str().unwrap(),
            "-f",
            "ETC2",
            "-c",
            "force",
            "-shrink",
            "2",
            "-maxmipmaps",
            "1",
        ]
    );
    assert!(request.output.exists());
}

#[test]
fn helper_diagnostics_are_bounded_and_drained() {
    let dir = tempdir().unwrap();
    let helper = install_helper(dir.path(), "compiler", "large-stderr", &[]);
    let request = request(dir.path(), Compression::HighQuality, Reduction::Original);

    let error = host_backend(&helper)
        .transcode_texture(&request)
        .unwrap_err();
    let diagnostic = error.to_string();

    assert_eq!(error.code(), ErrorCode::ConversionFailed);
    assert!(diagnostic.contains("[truncated]"));
    assert!(
        diagnostic.len() < 10_000,
        "diagnostic was {} bytes",
        diagnostic.len()
    );
    let arguments = only_invocation(&helper);
    assert_owned_temp_output(&arguments, &request.output);
}

#[test]
fn concurrent_failure_cannot_delete_another_calls_published_output() {
    let dir = tempdir().unwrap();
    let a_started = dir.path().join("a-started");
    let b_started = dir.path().join("b-started");
    let a_complete = dir.path().join("a-complete");
    let helper_a = install_helper(
        dir.path(),
        "compiler-a",
        "wait-success",
        &[&a_started, &b_started],
    );
    let helper_b = install_helper(
        dir.path(),
        "compiler-b",
        "signal-wait-fail",
        &[&b_started, &a_complete],
    );
    let request = request(dir.path(), Compression::HighQuality, Reduction::Original);

    let request_a = request.clone();
    let helper_a_for_thread = helper_a.clone();
    let a = thread::spawn(move || host_backend(&helper_a_for_thread).transcode_texture(&request_a));
    wait_for_path(&a_started);

    let request_b = request.clone();
    let helper_b_for_thread = helper_b.clone();
    let b = thread::spawn(move || host_backend(&helper_b_for_thread).transcode_texture(&request_b));

    let a_result = a.join().unwrap();
    assert!(a_result.is_ok(), "first conversion failed: {a_result:?}");
    fs::write(&a_complete, b"complete").unwrap();

    let b_error = b.join().unwrap().unwrap_err();
    assert_eq!(b_error.code(), ErrorCode::ConversionFailed);
    assert_eq!(fs::read(&request.output).unwrap(), b"TEXV0005\0converted");

    let temporary_a = assert_owned_temp_output(&only_invocation(&helper_a), &request.output);
    let temporary_b = assert_owned_temp_output(&only_invocation(&helper_b), &request.output);
    assert_ne!(temporary_a, temporary_b);
}

#[test]
fn concurrent_success_cannot_replace_another_calls_published_output() {
    let dir = tempdir().unwrap();
    let a_started = dir.path().join("a-started");
    let b_started = dir.path().join("b-started");
    let a_complete = dir.path().join("a-complete");
    let helper_a = install_helper(
        dir.path(),
        "compiler-a",
        "wait-success",
        &[&a_started, &b_started],
    );
    let helper_b = install_helper(
        dir.path(),
        "compiler-b",
        "signal-wait-success",
        &[&b_started, &a_complete],
    );
    let request = request(dir.path(), Compression::HighQuality, Reduction::Original);

    let request_a = request.clone();
    let helper_a_for_thread = helper_a.clone();
    let a = thread::spawn(move || host_backend(&helper_a_for_thread).transcode_texture(&request_a));
    wait_for_path(&a_started);

    let request_b = request.clone();
    let helper_b_for_thread = helper_b.clone();
    let b = thread::spawn(move || host_backend(&helper_b_for_thread).transcode_texture(&request_b));

    let a_result = a.join().unwrap();
    assert!(a_result.is_ok(), "first conversion failed: {a_result:?}");
    fs::write(&a_complete, b"complete").unwrap();

    let b_error = b.join().unwrap().unwrap_err();
    assert_eq!(b_error.code(), ErrorCode::ConversionFailed);
    assert!(b_error.to_string().contains("already exists"));
    assert_eq!(fs::read(&request.output).unwrap(), b"TEXV0005\0converted");

    let temporary_a = assert_owned_temp_output(&only_invocation(&helper_a), &request.output);
    let temporary_b = assert_owned_temp_output(&only_invocation(&helper_b), &request.output);
    assert_ne!(temporary_a, temporary_b);
}
