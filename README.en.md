# pkg2mpkg

**Language / 语言:** [中文](README.md) | English

`pkg2mpkg` is a cross-platform (macOS, Windows, Linux) Wallpaper Engine mobile package (`.mpkg`) tool. The core is implemented in Rust. The goal is to reimplement Windows Wallpaper Engine 2.8.26 `exportMobilePkg` and stay compatible with the unmodified official Wallpaper Engine Android 2.8.8 client.

This release is a **phase-1 runnable foundation**, not a full converter: project inspection, profile decisions, MPKG container read/write, CLI, and real export (texture conversion) for loose and packaged dynamic Scenes are available. Video, Scene pre-render, ADB Auto, and a desktop GUI are later stages. Wallpaper types without an execution backend return `backend_unavailable` on non-`--dry-run` export instead of producing fake or partial output.

> The GitHub repository is named `pkg2pkgm`; the CLI and crate name is `pkg2mpkg` (package → mobile package).

## Evidence boundary

- Windows behavior is taken from a local Wallpaper Engine **2.8.26** install tree, not incomplete third-party reverse notes.
- The Android target is official `io.wallpaperengine.weclient` **2.8.8**.
- Official runtimes, APKs, Wine, and proprietary Workshop samples are **local oracles only** and are not committed. See [reference/README.md](reference/README.md).

## Current capabilities

- Inspect project directories, `project.json`, `.pkg` inputs that resolve to a project manifest, and direct MP4/WebM files.
- Supports Scene and Video; explicit or inferred Web/Application returns exit code `3`.
- Preserves unknown `project.json` fields; never mutates the source project.
- Replicates the Windows 2.8.26 High / Balanced / Performance Scene profile matrix.
- Classifies Pixel Art / Normal / UHD using Windows thresholds: Scene area &lt; `307200` → Pixel Art; resolution-label area &gt; `2075520` prefers UHD.
- Library support for arbitrary manual Cover and KeepAspect geometry. H.264 output requires positive even dimensions; odd input media is allowed.
- Safe **read** of any `PKGM` + 4-digit version (including third-party/legacy **PKGM0014**–**PKGM0020**); rejects path traversal, duplicates, out-of-range, overlap, invalid UTF-8, and truncated directories.
- `inspect` / `verify` work directly on `.mpkg` (video/scene); Web/Application still rejected (exit `3`).
- Deterministic **write** of PKGM0018/PKGM0020 only (export default 0020), under 4 GiB; same-dir partials, self-check, atomic publish.
- `export --dry-run` emits a stable `ExportPlan` JSON and lists helper capabilities required for real execution.

## Install

### Prebuilt packages (recommended)

GitHub Actions publishes ready-to-run binaries for each release:

| Platform | Arch | Asset pattern |
|---|---|---|
| Linux | x86_64 / aarch64 | `pkg2mpkg-v*-x86_64-unknown-linux-gnu.tar.gz`, etc. |
| macOS | Apple Silicon / Intel | `…-aarch64-apple-darwin.tar.gz` / `…-x86_64-apple-darwin.tar.gz` |
| Windows | x86_64 | `…-x86_64-pc-windows-msvc.zip` |

Releases: https://github.com/anpplex/pkg2pkgm/releases

**One-line install (macOS / Linux → `/usr/local/bin`):**

```bash
curl -fsSL https://raw.githubusercontent.com/anpplex/pkg2pkgm/main/scripts/install.sh | bash
# Or pin version / install dir:
# VERSION=v0.1.0 INSTALL_DIR=~/.local/bin bash <(curl -fsSL …/scripts/install.sh)
```

**Manual extract:**

```bash
# Linux / macOS
tar -xzf pkg2mpkg-v0.1.0-<target>.tar.gz
cd pkg2mpkg-v0.1.0-<target>
./pkg2mpkg --help
```

```powershell
# Windows
Expand-Archive pkg2mpkg-v0.1.0-x86_64-pc-windows-msvc.zip
.\pkg2mpkg-v0.1.0-x86_64-pc-windows-msvc\pkg2mpkg.exe --help
```

### Build from source

Requires Rust **1.97.0** (pinned by `rust-toolchain.toml`).

```bash
cargo build --release -p pkg2mpkg --bin pkg2mpkg
./target/release/pkg2mpkg --help
```

Inspect a Scene project:

```bash
cargo run -p pkg2mpkg -- inspect /path/to/scene_project --json
```

Read-only export plan for a dynamic Scene:

```bash
cargo run -p pkg2mpkg -- export /path/to/scene \
  --output /tmp/scene.mpkg \
  --profile balanced \
  --dry-run \
  --json
```

Verify an official or generated MPKG:

```bash
cargo run -p pkg2mpkg -- verify /path/to/wallpaper.mpkg --json
```

Scenes require an explicit `high`, `balanced`, or `performance` profile. Video is planned conservatively as H.264 re-encode in this phase; passthrough is only allowed once a future probe proves container, codec, pixel format, rotation, size, FPS, and audio are all compatible.

## Exporting a loose Scene

Real (non-`--dry-run`) export of a **loose** dynamic Scene needs a Wallpaper Engine 2.8.26 runtime; on non-Windows hosts, Wine is also required:

```bash
# macOS / Linux
cargo run -p pkg2mpkg -- export /path/to/loose_scene \
  --output /tmp/scene.mpkg \
  --profile high \
  --we-runtime "/path/to/Wallpaper Engine.2.8.26" \
  --wine /path/to/wine \
  --replace \
  --json

# Windows: omit --wine / --winepath; launch resourcecompiler64.exe natively
cargo run -p pkg2mpkg -- export C:\path\to\loose_scene \
  --output C:\temp\scene.mpkg \
  --profile high \
  --we-runtime "C:\path\to\Wallpaper Engine.2.8.26" \
  --replace
```

### zcompat shaders (automatic with `--we-runtime`)

When `--we-runtime` is set and `assets/zcompat/scene/shaders/` exists under the runtime tree, dynamic export **automatically** applies the compatibility shader override policy (match by workshop project id / `maximumprojectid` and replace named frag/vert). Missing directory is a silent no-op; no extra CLI flag.

### Wine / winepath notes (macOS / Linux)

- `--wine` is required for dynamic Scene export on non-Windows; `--winepath` is optional and defaults to the `winepath` sibling of `WE_WINE`.
- **Wine Stable’s `winepath` is often a symlink to `wine`**, and selects winepath behavior via **argv0 basename**. Canonicalizing that link turns the invocation into `wine -w …` and fails via ShellExecuteEx. The CLI keeps the final path component of absolute paths (does not resolve away the name `winepath`). Custom `--winepath` should still be a path whose basename is `winepath`.
- Wine prefix, temp paths, and helper calls are **per-task ephemeral state** and are not written into the output MPKG.

Official Android 2.8.8 device qualification is outside the hermetic default quality gate, but is required before claiming Android behavioral compatibility. See [reference/README.md](reference/README.md).

## Packaged scene.pkg

Real Workshop Scenes appear in two forms: `project.json` declares `file: scene.pkg` directly; or it still declares `file: scene.json` while the loose JSON is missing and a same-stem sibling `scene.pkg` exists. The latter keeps the logical manifest entry and resolves the physical input to that exact sibling. Loose JSON always wins when present. Android export must unpack to a loose Scene tree first; the raw `.pkg` must **not** be packed into the Android MPKG.

### Status

| Layer | Status |
|---|---|
| Detection | `inspect` uses the resolved physical entry: direct `*.pkg`, and exact sibling `scene.pkg` when `scene.json` is missing |
| Boundary API | `ScenePackageUnpackBackend`, `unpack_scene_package_checked`, `prepare_packaged_scene_source` |
| Native impl | `DesktopPackageArchive` + `NativeScenePackageUnpackBackend` (in core, **no Wine**) |
| Container | Desktop `PKGV****` and Android `PKGM*` share the same table layout; only the 8-byte magic differs |
| Pipeline | Unpack into task-private `unpacked/`, then inventory / TEX conversion |
| CLI | Dynamic Scene export injects `NativeScenePackageUnpackBackend`; loose projects are a no-op |
| Safety | Path normalization and size limits aligned with MPKG; **forbids** writing unpacked `scene.pkg` into Android MPKG |

Library consumers may inject a custom `ScenePackageUnpackBackend`; without one, packaged sources return `BackendUnavailable(scene_pkg_unpack)` (exit `5`).

### CLI export of packaged projects

Same flags as loose Scenes:

```bash
pkg2mpkg export /path/to/workshop-project \
  --output out.mpkg \
  --profile high \
  --we-runtime '/path/to/Wallpaper Engine.2.8.26' \
  --wine /path/to/wine --winepath /path/to/winepath   # non-Windows
```

Pipeline: `scene.pkg` → loose Scene tree → convert `.tex` → PKGM0020.

## CLI exit codes

| Code | Meaning |
|---:|---|
| `0` | Success |
| `2` | Invalid arguments or plan |
| `3` | Web/Application not supported for mobile export |
| `4` | Invalid project or MPKG |
| `5` | Required export backend unavailable |
| `6` | Conversion failure |
| `7` | Output I/O or 4 GiB limit |
| `8` | Output verification failure |
| `9` | Device operation failure |
| `130` | Cancelled |

With `--json`, business errors go to stderr:

```json
{
  "code": "unsupported_wallpaper_type",
  "stage": "inspect",
  "message": "unsupported wallpaper type: web"
}
```

## Tests

Full quality gate:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The default suite is **hermetic**: no Wine, no official resourcecompiler, no APK, no proprietary assets.

### Local Dino dynamic Scene oracle (default `#[ignore]`)

With a Windows WE 2.8.26 tree, Wine (macOS/Linux), and optionally the official Android Dino MPKG:

```bash
WE_RUNTIME='/path/to/Wallpaper Engine.2.8.26' \
WE_DINO_PROJECT="$WE_RUNTIME/projects/defaultprojects/dino_run" \
WE_WINE=/path/to/wine \
WE_DINO_MPKG=/tmp/dino_run.mpkg \
  cargo test -p pkg2mpkg-core --test dino_dynamic_reference -- --ignored --nocapture
```

| Variable | Role |
|---|---|
| `WE_RUNTIME` | WE 2.8.26 install root (`distribution/bin/resourcecompiler64.exe`) |
| `WE_DINO_PROJECT` | Loose Dino Scene project directory |
| `WE_WINE` | Wine binary (required on non-Windows) |
| `WE_WINEPATH` | Optional; defaults to `winepath` sibling of `WE_WINE` |
| `WE_DINO_MPKG` | Optional official Android Dino MPKG for structural checks |

Full recipe: [reference/README.md](reference/README.md).

### Official Android Dino MPKG (read-only)

```bash
WE_DINO_MPKG=/tmp/dino_run.mpkg \
  cargo test -p pkg2mpkg-fixtures --test reference_dino -- --ignored
```

## Repository layout

```
.
├── Cargo.toml              # workspace
├── crates/
│   ├── cli/                # pkg2mpkg binary
│   ├── core/               # inspect / plan / MPKG / Scene pipeline
│   ├── codecs/             # resourcecompiler + Wine adapters
│   └── fixtures/           # test fixtures and reference readers
├── reference/              # local oracle docs (no binaries)
├── README.md               # Chinese (default)
└── README.en.md            # English
```

## Roadmap

1. Video and Scene pre-render: FFmpeg, arbitrary resolution/FPS/crop, SceneCaptureBackend, Windows duration differentials.
2. ADB and desktop GUI: device auto-resolution, transfer acceptance, egui workflow, three-platform packages.

The project will not claim a full `exportMobilePkg` reimplementation until dynamic Scenes pass official Android 2.8.8 device qualification and the Video/GUI stages land.

## License

MIT
