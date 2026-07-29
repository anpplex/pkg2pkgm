//! Local proprietary Dino Run oracle (Task 8).
//!
//! These tests are ignored by default so CI and hermetic runs never require
//! the research Wallpaper Engine runtime, Wine, or official Android MPKG.
//!
//! Required environment (never commit the artifacts they point at):
//! - `WE_RUNTIME` — Windows WE 2.8.26 install root (contains
//!   `distribution/bin/resourcecompiler64.exe`)
//! - `WE_DINO_PROJECT` — loose Dino Scene directory
//! - `WE_WINE` — Wine binary (required on non-Windows hosts)
//! - `WE_WINEPATH` — optional winepath; defaults to sibling of `WE_WINE`
//! - `WE_DINO_MPKG` — optional official Android Dino MPKG for structural compare
//!
//! Run:
//! ```text
//! WE_RUNTIME='…/research/Wallpaper Engine.2.8.26' \
//! WE_DINO_PROJECT="$WE_RUNTIME/projects/defaultprojects/dino_run" \
//! WE_WINE=/path/to/wine \
//! cargo test -p pkg2mpkg-core --test dino_dynamic_reference -- --ignored --nocapture
//! ```

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use pkg2mpkg_codecs::ResourceCompilerBackend;
use pkg2mpkg_core::{
    Compression, ContainerVersion, ContentClass, ExportContext, ExportMode, ExportRequest,
    MpkgArchive, OverwritePolicy, ProjectManifest, Reduction, ResourceTranscodeBackend,
    SceneProfile, TextureTranscodeRequest, WallpaperKind, build_export_plan, execute_export_plan,
    inspect_source,
};
use tempfile::tempdir;

const TEX_V5: &[u8; 9] = b"TEXV0005\0";
const TEX_I1: &[u8; 9] = b"TEXI0001\0";
const TEX_B4: &[u8; 8] = b"TEXB0004";
const TEX_S3: &[u8; 8] = b"TEXS0003";
const EXPECTED_TEX_COUNT: usize = 32;
const COMPILER_RELATIVE: &str = "distribution/bin/resourcecompiler64.exe";

/// Minimal structural view of a Wallpaper Engine TEX used by the oracle.
#[derive(Debug, Clone)]
struct TexStructure {
    format: u32,
    flags: u32,
    logical_width: u32,
    logical_height: u32,
    image_width: u32,
    image_height: u32,
    container_is_b4: bool,
    image_count: u32,
    free_image_format: i32,
    mip_count: u32,
    mip_width: u32,
    mip_height: u32,
    has_sprite_suffix: bool,
}

fn require_env(name: &str) -> PathBuf {
    let value = env::var(name).unwrap_or_else(|_| {
        panic!(
            "set {name} to run the Dino dynamic reference oracle (see pkg2mpkg/reference/README.md)"
        )
    });
    let path = PathBuf::from(value);
    assert!(
        path.exists(),
        "{name} path does not exist: {}",
        path.display()
    );
    path
}

fn optional_env(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.exists())
}

fn dino_project() -> PathBuf {
    require_env("WE_DINO_PROJECT")
}

fn we_runtime() -> PathBuf {
    require_env("WE_RUNTIME")
}

fn resource_compiler(runtime: &Path) -> PathBuf {
    let compiler = runtime.join(COMPILER_RELATIVE);
    assert!(
        compiler.is_file(),
        "resourcecompiler64.exe missing under WE_RUNTIME: {}",
        compiler.display()
    );
    compiler
}

fn build_backend(runtime: &Path) -> ResourceCompilerBackend {
    let compiler = resource_compiler(runtime);

    #[cfg(windows)]
    {
        let _ = env::var_os("WE_WINE");
        ResourceCompilerBackend::native(compiler)
    }

    #[cfg(not(windows))]
    {
        let wine = require_env("WE_WINE");
        let winepath = match optional_env("WE_WINEPATH") {
            Some(path) => path,
            None => {
                let sibling = wine
                    .parent()
                    .expect("WE_WINE must have a parent directory")
                    .join("winepath");
                assert!(
                    sibling.is_file(),
                    "winepath not found beside WE_WINE at {}; set WE_WINEPATH",
                    sibling.display()
                );
                sibling
            }
        };
        ResourceCompilerBackend::wine(compiler, wine, winepath)
    }
}

fn list_project_tex_files(project: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![project.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap_or_else(|error| {
            panic!("cannot read {}: {error}", dir.display());
        }) {
            let entry = entry.unwrap();
            let path = entry.path();
            let file_type = entry.file_type().unwrap();
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file()
                && path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("tex"))
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..offset + 4)
        .map(|slice| u32::from_le_bytes(slice.try_into().unwrap()))
}

fn read_i32_le(bytes: &[u8], offset: usize) -> Option<i32> {
    bytes
        .get(offset..offset + 4)
        .map(|slice| i32::from_le_bytes(slice.try_into().unwrap()))
}

/// Parse TEXV0005 / TEXI0001 / first TEXB block structurally.
///
/// Pixel decode is intentionally not performed here: the local suite has no
/// ETC2/RGBA decode helpers wired into pkg2mpkg-core, so the oracle asserts
/// container layout, dimensions, format id, and sprite suffix survival only.
fn parse_tex_structure(bytes: &[u8]) -> Result<TexStructure, String> {
    if bytes.len() < 50 {
        return Err(format!("TEX too short ({} bytes)", bytes.len()));
    }
    if &bytes[..9] != TEX_V5 {
        return Err("missing TEXV0005\\0 magic".into());
    }
    if &bytes[9..18] != TEX_I1 {
        return Err("missing TEXI0001\\0 image header".into());
    }

    let format = read_u32_le(bytes, 18).ok_or("truncated TEXI format")?;
    let flags = read_u32_le(bytes, 22).ok_or("truncated TEXI flags")?;
    let logical_width = read_u32_le(bytes, 26).ok_or("truncated TEXI width")?;
    let logical_height = read_u32_le(bytes, 30).ok_or("truncated TEXI height")?;
    let image_width = read_u32_le(bytes, 34).ok_or("truncated TEXI image width")?;
    let image_height = read_u32_le(bytes, 38).ok_or("truncated TEXI image height")?;

    let texb = bytes
        .windows(4)
        .position(|window| window == b"TEXB")
        .ok_or("missing TEXB container")?;
    if texb + 8 > bytes.len() {
        return Err("truncated TEXB magic".into());
    }
    let container = &bytes[texb..texb + 8];
    let container_is_b4 = container == TEX_B4 || container.starts_with(b"TEXB0004");
    if !container.starts_with(b"TEXB0003") && !container.starts_with(b"TEXB0004") {
        return Err(format!("unexpected TEXB magic {container:?}"));
    }

    let mut offset = texb + 8;
    if bytes.get(offset) == Some(&0) {
        offset += 1;
    }
    let image_count = read_u32_le(bytes, offset).ok_or("truncated image_count")?;
    offset += 4;
    let free_image_format = read_i32_le(bytes, offset).ok_or("truncated free_image_format")?;
    offset += 4;
    if container.starts_with(b"TEXB0004") {
        offset += 4; // TEXB0004 extra field
    }
    let mip_count = read_u32_le(bytes, offset).ok_or("truncated mip_count")?;
    offset += 4;
    let mip_width = read_u32_le(bytes, offset).ok_or("truncated mip width")?;
    offset += 4;
    let mip_height = read_u32_le(bytes, offset).ok_or("truncated mip height")?;

    Ok(TexStructure {
        format,
        flags,
        logical_width,
        logical_height,
        image_width,
        image_height,
        container_is_b4,
        image_count,
        free_image_format,
        mip_count,
        mip_width,
        mip_height,
        has_sprite_suffix: find_bytes(bytes, TEX_S3),
    })
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn shrink_factor(reduction: Reduction) -> u32 {
    match reduction {
        Reduction::Original => 1,
        Reduction::X2 => 2,
        Reduction::X4 => 4,
    }
}

fn expected_mip_dim(logical: u32, shrink: u32) -> u32 {
    (logical / shrink).max(1)
}

fn archive_paths(archive: &MpkgArchive) -> Vec<String> {
    archive
        .entries()
        .iter()
        .map(|entry| entry.path.clone())
        .collect()
}

fn assert_no_editor_sidecars(paths: &[String]) {
    for path in paths {
        let lower = path.to_ascii_lowercase();
        assert!(
            !lower.ends_with(".tex-json"),
            "editor sidecar leaked into MPKG: {path}"
        );
        assert!(
            !lower.ends_with(".mpkg"),
            "nested mpkg leaked into MPKG: {path}"
        );
        assert!(
            !lower.ends_with(".partial"),
            "partial artifact leaked into MPKG: {path}"
        );
        assert!(
            !path
                .split('/')
                .any(|component| component.to_ascii_lowercase().starts_with(".pkg2mpkg-")),
            "task temp path leaked into MPKG: {path}"
        );
    }
}

fn assert_required_dino_assets(paths: &[String]) {
    let required_prefixes_or_paths = [
        "project.json",
        "scene.json",
        "materials/",
        "models/",
        "particles/",
        "shaders/",
        "sounds/",
        "effects/",
    ];
    for required in required_prefixes_or_paths {
        assert!(
            paths
                .iter()
                .any(|path| path == required || path.starts_with(required)),
            "MPKG missing required Scene asset group {required}; got {paths:?}"
        );
    }

    // Concrete well-known Dino references that must survive packaging.
    for required in [
        "materials/vita_walk_01.tex",
        "materials/coin_0.tex",
        "models/vita_walk.json",
        "particles/coinget.json",
        "sounds/dino_jump.wav",
        "sounds/coin2.wav",
        "shaders/effects/scroll.frag",
        "effects/godrays/effect.json",
        "effects/scroll/effect.json",
    ] {
        assert!(
            paths.iter().any(|path| path == required),
            "MPKG missing required Dino path {required}"
        );
    }
}

/// Convert every Dino `.tex` with the real resource compiler and assert the
/// Windows 2.8.26 structural contract for High Quality / shrink 1.
#[test]
#[ignore = "requires WE_RUNTIME, WE_DINO_PROJECT, and WE_WINE (unix) for the local Dino oracle"]
fn dino_all_32_tex_convert_structurally() {
    let runtime = we_runtime();
    let project = dino_project();
    let backend = build_backend(&runtime);
    let tex_files = list_project_tex_files(&project);
    assert_eq!(
        tex_files.len(),
        EXPECTED_TEX_COUNT,
        "Dino oracle expects {EXPECTED_TEX_COUNT} TEX files, found {}",
        tex_files.len()
    );

    let out_dir = tempdir().unwrap();
    let compression = Compression::HighQuality;
    let reduction = Reduction::Original;
    let shrink = shrink_factor(reduction);
    let mut sprite_survivors = 0_usize;
    let mut conversion_count = 0_usize;

    for input in &tex_files {
        let relative = input.strip_prefix(&project).unwrap_or(input.as_path());
        let output = out_dir.path().join(relative);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).unwrap();
        }

        let source_bytes = fs::read(input).unwrap();
        let source = parse_tex_structure(&source_bytes).unwrap_or_else(|error| {
            panic!("cannot parse source TEX {}: {error}", input.display());
        });

        let request =
            TextureTranscodeRequest::new(input.clone(), output.clone(), compression, reduction)
                .unwrap();
        let report = backend.transcode_texture(&request).unwrap_or_else(|error| {
            panic!(
                "resource compiler failed for {}: {error}",
                relative.display()
            )
        });
        assert_eq!(report.output, output);
        assert_eq!(report.compression, compression);
        assert_eq!(report.reduction, reduction);
        assert!(report.output_bytes > 0);

        let converted_bytes = fs::read(&output).unwrap();
        assert!(
            converted_bytes.starts_with(TEX_V5),
            "{} output missing TEXV0005\\0",
            relative.display()
        );

        let structure = parse_tex_structure(&converted_bytes).unwrap_or_else(|error| {
            panic!("cannot parse converted TEX {}: {error}", relative.display());
        });

        assert!(
            structure.container_is_b4,
            "{} expected TEXB0004 container after conversion",
            relative.display()
        );
        assert_eq!(
            structure.mip_count,
            1,
            "{} expected exactly one mip (-maxmipmaps 1)",
            relative.display()
        );
        // Measured Windows 2.8.26 behavior: desktop TEX may store a power-of-two
        // TEXI "logical" size larger than the content `image_*` size (e.g. Dino
        // b1_0.tex is logical 512x256 with content 272x160). After mobile
        // transcode, TEXI logical fields match the content dimensions, not the
        // padded desktop logical size. Prefer image_* as the content baseline.
        let content_width = source.image_width;
        let content_height = source.image_height;
        assert_eq!(
            structure.logical_width,
            content_width,
            "{} mobile logical width should match desktop content width (desktop logical was {}x{}, content {}x{})",
            relative.display(),
            source.logical_width,
            source.logical_height,
            content_width,
            content_height
        );
        assert_eq!(
            structure.logical_height,
            content_height,
            "{} mobile logical height should match desktop content height",
            relative.display()
        );
        assert_eq!(
            structure.image_width,
            content_width,
            "{} mobile image width should match content width",
            relative.display()
        );
        assert_eq!(
            structure.image_height,
            content_height,
            "{} mobile image height should match content height",
            relative.display()
        );
        assert_eq!(
            structure.mip_width,
            expected_mip_dim(content_width, shrink),
            "{} mip width mismatch for shrink {shrink}",
            relative.display()
        );
        assert_eq!(
            structure.mip_height,
            expected_mip_dim(content_height, shrink),
            "{} mip height mismatch for shrink {shrink}",
            relative.display()
        );
        assert_eq!(
            structure.image_count,
            1,
            "{} unexpected image_count",
            relative.display()
        );
        // High Quality omits -c force: raw RGBA stays format 0; already-compressed
        // desktop textures may still become format 5. Accept either without
        // claiming full pixel equality (no decode helper in this crate).
        assert!(
            structure.format == 0 || structure.format == 5,
            "{} unexpected format {} (expected 0 or 5 under High Quality)",
            relative.display(),
            structure.format
        );
        let _ = (
            structure.flags,
            structure.image_width,
            structure.image_height,
        );
        let _ = structure.free_image_format;

        if source.has_sprite_suffix {
            assert!(
                structure.has_sprite_suffix,
                "{} lost TEXS0003 sprite suffix after conversion",
                relative.display()
            );
            sprite_survivors += 1;
        }

        conversion_count += 1;
        eprintln!(
            "ok {} fmt={} logical={}x{} mip={}x{} sprite={}",
            relative.display(),
            structure.format,
            structure.logical_width,
            structure.logical_height,
            structure.mip_width,
            structure.mip_height,
            structure.has_sprite_suffix
        );
    }

    assert_eq!(conversion_count, EXPECTED_TEX_COUNT);
    assert!(
        sprite_survivors >= 1,
        "expected at least one sprite-bearing Dino TEX to survive conversion"
    );
    eprintln!(
        "converted {conversion_count} TEX files; sprite suffix survivors: {sprite_survivors}"
    );
}

/// Force ETC2 (`-c force`) on one Dino texture and assert mobile format 5 plus
/// structural dimensions. Full ETC2 pixel decode is out of scope without a
/// decode helper.
#[test]
#[ignore = "requires WE_RUNTIME, WE_DINO_PROJECT, and WE_WINE (unix) for the local Dino oracle"]
fn dino_forced_etc2_is_format_5() {
    let runtime = we_runtime();
    let project = dino_project();
    let backend = build_backend(&runtime);

    let input = project.join("materials/grass_ground.tex");
    assert!(
        input.is_file(),
        "expected Dino sample texture at {}",
        input.display()
    );
    let source = parse_tex_structure(&fs::read(&input).unwrap()).unwrap();

    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("grass_ground.tex");
    let request = TextureTranscodeRequest::new(
        input,
        output.clone(),
        Compression::HighPerformance,
        Reduction::Original,
    )
    .unwrap();

    backend.transcode_texture(&request).unwrap();
    let converted = parse_tex_structure(&fs::read(&output).unwrap()).unwrap();

    assert!(converted.container_is_b4);
    assert_eq!(converted.mip_count, 1);
    assert_eq!(converted.format, 5, "forced ETC2 must report format 5");
    assert_eq!(converted.logical_width, source.logical_width);
    assert_eq!(converted.logical_height, source.logical_height);
    assert_eq!(
        converted.mip_width,
        expected_mip_dim(source.logical_width, 1)
    );
    assert_eq!(
        converted.mip_height,
        expected_mip_dim(source.logical_height, 1)
    );
    eprintln!(
        "forced ETC2 grass_ground.tex format={} mip={}x{}",
        converted.format, converted.mip_width, converted.mip_height
    );
}

/// Full `execute_export_plan` for Dino: PKGM0020, 32 converted TEX, required
/// Scene/script/material/model/particle/sound/shader assets, no editor sidecars.
#[test]
#[ignore = "requires WE_RUNTIME, WE_DINO_PROJECT, and WE_WINE (unix) for the local Dino oracle"]
fn dino_execute_export_plan_packages_scene_assets() {
    let runtime = we_runtime();
    let project = dino_project();
    let backend = build_backend(&runtime);

    let source = inspect_source(&project).unwrap();
    assert_eq!(source.kind, WallpaperKind::Scene);
    assert_eq!(source.title, "Dino Run");

    let out_dir = tempdir().unwrap();
    let output = out_dir.path().join("dino_run.mpkg");
    // High + PixelArt (Dino) → HighQuality + Original, matching the Windows HQ path.
    let plan = build_export_plan(
        &source,
        ExportRequest::scene(output.clone(), SceneProfile::High, ContentClass::PixelArt),
    )
    .unwrap();
    assert!(matches!(
        plan.mode,
        ExportMode::SceneDynamic {
            compression: Compression::HighQuality,
            reduction: Reduction::Original,
        }
    ));

    let context = ExportContext::with_resource_backend(&backend);
    let report = execute_export_plan(&source, &plan, &context, OverwritePolicy::Deny)
        .unwrap_or_else(|error| panic!("execute_export_plan failed: {error}"));

    assert_eq!(report.kind, WallpaperKind::Scene);
    assert_eq!(report.container_version, ContainerVersion::Pkgm0020);
    assert_eq!(report.texture_count, EXPECTED_TEX_COUNT);
    assert_eq!(report.output, output);
    assert!(report.texture_input_bytes > 0);
    assert!(report.texture_output_bytes > 0);
    assert!(report.output_bytes > 0);
    assert!(report.entry_count > EXPECTED_TEX_COUNT);

    let archive = MpkgArchive::open(&output).unwrap();
    assert_eq!(archive.version(), ContainerVersion::Pkgm0020);

    let paths = archive_paths(&archive);
    let mut sorted = paths.clone();
    sorted.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    assert_eq!(paths, sorted, "archive entries must be bytewise ordered");

    assert_no_editor_sidecars(&paths);
    assert_required_dino_assets(&paths);

    let project_bytes = archive.read_entry("project.json").unwrap();
    let manifest = ProjectManifest::parse(&project_bytes).unwrap();
    assert_eq!(manifest.kind().unwrap(), WallpaperKind::Scene);
    assert_eq!(manifest.title(), Some("Dino Run"));

    // Converted TEX entries remain TEXV0005 and keep sprite suffixes where present.
    let mut packaged_tex = 0_usize;
    let mut packaged_sprites = 0_usize;
    for path in &paths {
        if !path.to_ascii_lowercase().ends_with(".tex") {
            continue;
        }
        let bytes = archive.read_entry(path).unwrap();
        assert!(bytes.starts_with(TEX_V5), "packaged {path} is not TEXV0005");
        let structure = parse_tex_structure(&bytes).unwrap_or_else(|error| {
            panic!("packaged {path} failed structural parse: {error}");
        });
        assert!(
            structure.container_is_b4,
            "packaged {path} expected TEXB0004"
        );
        assert_eq!(structure.mip_count, 1, "packaged {path} mip_count");
        if structure.has_sprite_suffix {
            packaged_sprites += 1;
        }
        packaged_tex += 1;
    }
    assert_eq!(packaged_tex, EXPECTED_TEX_COUNT);
    assert!(
        packaged_sprites >= 1,
        "expected sprite-bearing TEX inside final MPKG"
    );

    // Optional: open the official Android reference if the operator supplied it.
    // Official mobile Dino is historically PKGM0018; we only require it opens as
    // a Scene titled "Dino Run" and does not claim byte-identity with our V20 pack.
    if let Some(reference) = optional_env("WE_DINO_MPKG") {
        let official = MpkgArchive::open(&reference).unwrap_or_else(|error| {
            panic!(
                "WE_DINO_MPKG could not be opened at {}: {error}",
                reference.display()
            );
        });
        let official_project = official.read_entry("project.json").unwrap();
        let official_manifest = ProjectManifest::parse(&official_project).unwrap();
        assert_eq!(official_manifest.kind().unwrap(), WallpaperKind::Scene);
        assert_eq!(official_manifest.title(), Some("Dino Run"));
        let official_paths = archive_paths(&official);
        assert!(
            official_paths.iter().any(|path| path == "scene.json"),
            "official Dino MPKG missing scene.json"
        );
        eprintln!(
            "optional WE_DINO_MPKG ok: version={:?} entries={}",
            official.version(),
            official_paths.len()
        );
    } else {
        eprintln!("WE_DINO_MPKG not set; skipped official Android structural compare");
    }

    eprintln!(
        "dino export ok: entries={} tex={} sprites={} bytes={}",
        report.entry_count, report.texture_count, packaged_sprites, report.output_bytes
    );
}
