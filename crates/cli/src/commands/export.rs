use std::{
    fs,
    path::{Path, PathBuf},
};

use pkg2mpkg_codecs::ResourceCompilerBackend;
use pkg2mpkg_core::{
    Error, ExportContext, ExportMode, ExportRequest, NativeScenePackageUnpackBackend,
    OverwritePolicy, Result, VideoInputCompatibility, WallpaperKind, build_export_plan,
    classify_content_class, execute_export_plan, inspect_source,
};

use crate::{
    args::{CompressionArg, ProfileArg, ReductionArg, resolve_scene_profile_arg},
    output,
};

const COMPILER_RELATIVE: &str = "distribution/bin/resourcecompiler64.exe";

/// CLI options for `pkg2mpkg export` after clap parsing.
pub struct ExportOptions {
    pub output: PathBuf,
    pub profile: Option<ProfileArg>,
    pub compression: Option<CompressionArg>,
    pub reduction: Option<ReductionArg>,
    pub we_runtime: Option<PathBuf>,
    pub wine: Option<PathBuf>,
    pub winepath: Option<PathBuf>,
    pub replace: bool,
    pub dry_run: bool,
    pub json: bool,
}

pub fn run(input: &Path, options: ExportOptions) -> Result<()> {
    validate_export_flag_combinations(
        options.we_runtime.as_deref(),
        options.wine.as_deref(),
        options.winepath.as_deref(),
    )?;

    let source = inspect_source(input)?;
    let request = match source.kind {
        WallpaperKind::Scene => {
            let profile = options.profile.ok_or_else(|| Error::InvalidArguments {
                reason: "Scene export requires --profile <high|balanced|performance|custom>".into(),
            })?;
            let scene_profile =
                resolve_scene_profile_arg(profile, options.compression, options.reduction)
                    .map_err(|reason| Error::InvalidArguments { reason })?;
            let content_class = classify_content_class(&source)?;
            ExportRequest::scene(options.output, scene_profile, content_class)
        }
        WallpaperKind::Video => {
            if options.profile.is_some() {
                return Err(Error::InvalidArguments {
                    reason: "--profile is only valid for Scene export".into(),
                });
            }
            if options.compression.is_some() || options.reduction.is_some() {
                return Err(Error::InvalidArguments {
                    reason: "--compression and --reduction are only valid for Scene custom profile"
                        .into(),
                });
            }
            ExportRequest::video(options.output, VideoInputCompatibility::Unknown)
        }
        WallpaperKind::Web | WallpaperKind::Application => {
            return Err(Error::unsupported_type(source.kind.as_str()));
        }
    };

    let plan = build_export_plan(&source, request)?;

    // Dry-run returns before any runtime canonicalization, helper probe,
    // backend construction, or Wine/winepath execution.
    if options.dry_run {
        return if options.json {
            output::print_json(&plan)
        } else {
            output::print_text(&format!(
                "dry run: {} -> {}\nmode: {:?}",
                plan.source.display(),
                plan.output.display(),
                plan.mode
            ))
        };
    }

    let overwrite = if options.replace {
        OverwritePolicy::Replace
    } else {
        OverwritePolicy::Deny
    };

    let report = match plan.mode {
        ExportMode::SceneDynamic { .. } => {
            let backend = build_dynamic_backend(
                options.we_runtime.as_deref(),
                options.wine.as_deref(),
                options.winepath.as_deref(),
            )?;
            // Optional zcompat overlay: only when the configured runtime ships the
            // official Scene shader compatibility tree. Missing dir is a silent no-op.
            let zcompat_root = options
                .we_runtime
                .as_ref()
                .map(|runtime| runtime.join("assets/zcompat/scene/shaders"));
            // Always attach native PKGV unpack (no-op for loose Scene trees).
            let unpack = NativeScenePackageUnpackBackend::new();
            let mut context =
                ExportContext::with_resource_backend(&backend).package_unpack_backend(&unpack);
            if let Some(path) = zcompat_root.as_ref() {
                if path.is_dir() {
                    context = context.compat_shader_root(path.as_path());
                }
            }
            execute_export_plan(&source, &plan, &context, overwrite)?
        }
        ExportMode::ScenePreRenderedVideo | ExportMode::Video { .. } => {
            // Performance / Video remain unavailable without launching helpers.
            execute_export_plan(&source, &plan, &ExportContext::new(), overwrite)?
        }
    };

    if options.json {
        output::print_json(&report)
    } else {
        output::print_text(&format!(
            "exported: {} -> {}\nmode: {:?}\nentries: {}\noutput_bytes: {}\ntextures: {}",
            report.source.display(),
            report.output.display(),
            report.mode,
            report.entry_count,
            report.output_bytes,
            report.texture_count
        ))
    }
}

fn validate_export_flag_combinations(
    we_runtime: Option<&Path>,
    wine: Option<&Path>,
    winepath: Option<&Path>,
) -> Result<()> {
    if winepath.is_some() && wine.is_none() {
        return Err(Error::InvalidArguments {
            reason: "--winepath requires --wine".into(),
        });
    }
    if wine.is_some() && we_runtime.is_none() {
        return Err(Error::InvalidArguments {
            reason: "--wine requires --we-runtime".into(),
        });
    }

    #[cfg(windows)]
    {
        if wine.is_some() || winepath.is_some() {
            return Err(Error::InvalidArguments {
                reason: "--wine and --winepath are not supported on Windows (native launch only)"
                    .into(),
            });
        }
    }

    Ok(())
}

fn build_dynamic_backend(
    we_runtime: Option<&Path>,
    wine: Option<&Path>,
    winepath: Option<&Path>,
) -> Result<ResourceCompilerBackend> {
    let we_runtime = we_runtime.ok_or_else(|| Error::BackendUnavailable {
        backend: "Wallpaper Engine runtime (--we-runtime) is required for dynamic Scene export"
            .into(),
    })?;

    let compiler = resolve_resource_compiler(we_runtime)?;

    #[cfg(windows)]
    {
        let _ = wine;
        let _ = winepath;
        Ok(ResourceCompilerBackend::native(compiler))
    }

    #[cfg(not(windows))]
    {
        let wine = wine.ok_or_else(|| Error::BackendUnavailable {
            backend: "Wine (--wine) is required for dynamic Scene export on this platform".into(),
        })?;
        let wine = resolve_unix_executable(wine, "Wine runtime")?;
        let winepath = match winepath {
            Some(path) => resolve_unix_executable(path, "winepath")?,
            None => {
                let sibling = wine
                    .parent()
                    .ok_or_else(|| Error::BackendUnavailable {
                        backend: format!(
                            "cannot derive winepath sibling for Wine at {}",
                            wine.display()
                        ),
                    })?
                    .join("winepath");
                resolve_unix_executable(&sibling, "winepath")?
            }
        };
        Ok(ResourceCompilerBackend::wine(compiler, wine, winepath))
    }
}

fn resolve_resource_compiler(we_runtime: &Path) -> Result<PathBuf> {
    let runtime_root = canonicalize_existing(we_runtime, "Wallpaper Engine runtime")?;
    let runtime_meta = fs::metadata(&runtime_root).map_err(|source| Error::BackendUnavailable {
        backend: format!(
            "cannot access Wallpaper Engine runtime at {}: {source}",
            runtime_root.display()
        ),
    })?;
    if !runtime_meta.is_dir() {
        return Err(Error::BackendUnavailable {
            backend: format!(
                "Wallpaper Engine runtime is not a directory: {}",
                runtime_root.display()
            ),
        });
    }

    let compiler_candidate = runtime_root.join(COMPILER_RELATIVE);
    let compiler = match fs::canonicalize(&compiler_candidate) {
        Ok(path) => path,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::BackendUnavailable {
                backend: format!(
                    "resource compiler not found at {} (expected under --we-runtime)",
                    compiler_candidate.display()
                ),
            });
        }
        Err(source) => {
            return Err(Error::BackendUnavailable {
                backend: format!(
                    "cannot access resource compiler at {}: {source}",
                    compiler_candidate.display()
                ),
            });
        }
    };

    if !compiler.starts_with(&runtime_root) {
        return Err(Error::BackendUnavailable {
            backend: format!(
                "resource compiler escapes Wallpaper Engine runtime root ({} not under {})",
                compiler.display(),
                runtime_root.display()
            ),
        });
    }

    let meta = fs::metadata(&compiler).map_err(|source| Error::BackendUnavailable {
        backend: format!(
            "cannot access resource compiler at {}: {source}",
            compiler.display()
        ),
    })?;
    if !meta.is_file() {
        return Err(Error::BackendUnavailable {
            backend: format!(
                "resource compiler is not a regular file: {}",
                compiler.display()
            ),
        });
    }

    Ok(compiler)
}

fn canonicalize_existing(path: &Path, label: &str) -> Result<PathBuf> {
    fs::canonicalize(path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            Error::BackendUnavailable {
                backend: format!("{label} not found at {}", path.display()),
            }
        } else {
            Error::BackendUnavailable {
                backend: format!("cannot access {label} at {}: {source}", path.display()),
            }
        }
    })
}

#[cfg(not(windows))]
fn resolve_unix_executable(path: &Path, label: &str) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    // Wine Stable ships `winepath` as a symlink to `wine` and selects the
    // winepath personality from argv0's basename. Full canonicalize() would
    // resolve that symlink to `…/wine`, so `Command::new(winepath) -w …`
    // becomes `wine -w …` and fails with ShellExecuteEx. Keep an absolute
    // path that preserves the final path component; only the target needs to
    // be a regular executable file.
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let cwd = std::env::current_dir().map_err(|source| Error::BackendUnavailable {
            backend: format!("cannot resolve working directory for {label}: {source}"),
        })?;
        cwd.join(path)
    };

    let meta = fs::metadata(&absolute).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            Error::BackendUnavailable {
                backend: format!("{label} not found at {}", absolute.display()),
            }
        } else {
            Error::BackendUnavailable {
                backend: format!("cannot access {label} at {}: {source}", absolute.display()),
            }
        }
    })?;
    if !meta.is_file() {
        return Err(Error::BackendUnavailable {
            backend: format!("{label} is not a regular file: {}", absolute.display()),
        });
    }
    if meta.permissions().mode() & 0o111 == 0 {
        return Err(Error::BackendUnavailable {
            backend: format!("{label} is not executable: {}", absolute.display()),
        });
    }
    Ok(absolute)
}
