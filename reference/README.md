# Local official Android and Windows runtime references

Official APKs, extracted MPKGs, the Windows Wallpaper Engine runtime, Wine, and
generated conversion outputs are **local test oracles only**. Keep them outside
git; never add them to the repository.

## Official Android Dino MPKG (read-only)

Extract the bundled Dino Run package from a locally obtained official Wallpaper
Engine Android APK:

~~~bash
mkdir -p /tmp/pkg2mpkg-reference
unzip -p /path/to/io.wallpaperengine.weclient/base.apk \
  assets/wallpapers/dino_run.mpkg \
  > /tmp/pkg2mpkg-reference/dino_run.mpkg
WE_DINO_MPKG=/tmp/pkg2mpkg-reference/dino_run.mpkg \
  cargo test -p pkg2mpkg-fixtures --test reference_dino -- --ignored
~~~

The fixture test checks the real package through the same public reader and
manifest APIs used by the CLI. Normal test runs leave it ignored so builds remain
reproducible without proprietary artifacts.

## Dynamic Scene Dino oracle (resource compiler + Wine)

Task 8 adds an ignored integration test that drives the real
`resourcecompiler64.exe` (via Wine on macOS/Linux) against the research Dino
project and packages a PKGM0020 through `execute_export_plan`.

### Environment

| Variable | Required | Role |
|---|---|---|
| `WE_RUNTIME` | yes | Windows WE **2.8.26** install root containing `distribution/bin/resourcecompiler64.exe` |
| `WE_DINO_PROJECT` | yes | Loose Dino Scene directory (typically `$WE_RUNTIME/projects/defaultprojects/dino_run`) |
| `WE_WINE` | yes on unix | Wine binary used to launch the Windows compiler |
| `WE_WINEPATH` | no | Defaults to the `winepath` sibling of `WE_WINE` |
| `WE_DINO_MPKG` | no | Official Android Dino MPKG for structural title/type/scene checks only |

### Example (macOS / Linux)

~~~bash
export WE_RUNTIME='/Users/anpple/Codex/WallpaperEngine/research/Wallpaper Engine.2.8.26'
export WE_DINO_PROJECT="$WE_RUNTIME/projects/defaultprojects/dino_run"
export WE_WINE=/path/to/wine
# optional:
# export WE_WINEPATH=/path/to/winepath
# export WE_DINO_MPKG=/tmp/dino_run.mpkg

cargo test -p pkg2mpkg-core --test dino_dynamic_reference -- --ignored --nocapture
~~~

### Example (Windows)

On a Windows host, omit `WE_WINE` / `WE_WINEPATH`. The backend launches
`resourcecompiler64.exe` natively when `WE_RUNTIME` points at a 2.8.26 tree.

~~~powershell
$env:WE_RUNTIME = 'C:\path\to\Wallpaper Engine.2.8.26'
$env:WE_DINO_PROJECT = "$env:WE_RUNTIME\projects\defaultprojects\dino_run"
cargo test -p pkg2mpkg-core --test dino_dynamic_reference -- --ignored --nocapture
~~~

### What the oracle asserts

- Exactly **32** Dino `.tex` files convert through `ResourceCompilerBackend`.
- Converted files begin with `TEXV0005\0`, carry `TEXI0001`, use a `TEXB0004`
  container, keep **one** mip (`-maxmipmaps 1`), preserve logical TEXI dimensions,
  and set first-mip size to `logical / shrink` (shrink `1` for High Quality /
  Original).
- High Performance (`-c force`) sample conversion reports **format 5** (ETC2).
- Source textures that contain a `TEXS0003` sprite suffix retain it after
  conversion.
- `execute_export_plan` publishes **PKGM0020** with Scene/material/model/particle/
  sound/shader assets, no `.tex-json` / `.partial` / `.pkg2mpkg-*` sidecars.
- Optional `WE_DINO_MPKG` must open as a Scene titled `Dino Run` (official packs
  may be PKGM0018; no byte-identity claim against the generated PKGM0020).

### What it deliberately does **not** claim

- Pixel-exact RGBA equality against desktop crops (no ETC2/RGBA decode helper is
  wired into `pkg2mpkg-core` for this suite).
- Byte-identity with the APK-bundled Android MPKG.
- Device import/apply success (Android 2.8.8 smoke remains an operator step via
  ADB; see the Task 8 report).

### CLI equivalent

Once Wine and the runtime are configured, the same conversion path is available
from the public CLI:

~~~bash
cargo run -p pkg2mpkg --bin pkg2mpkg -- export "$WE_DINO_PROJECT" \
  --output /tmp/dino_run_generated.mpkg \
  --profile high \
  --we-runtime "$WE_RUNTIME" \
  --wine "$WE_WINE" \
  --replace \
  --json
~~~
