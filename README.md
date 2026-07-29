# pkg2mpkg

**语言 / Language:** 中文 | [English](README.en.md)

`pkg2mpkg` 是跨 macOS、Windows 和 Linux 的 Wallpaper Engine 移动包（`.mpkg`）工具。核心使用 Rust 实现，目标是复刻 Windows Wallpaper Engine 2.8.26 的 `exportMobilePkg`，并兼容未修改的官方 Wallpaper Engine Android 2.8.8。

当前版本是第 1 阶段可运行基础，不是完整转换器：项目检查、配置决策、MPKG 容器读写、CLI，以及松散与打包动态 Scene 的纹理转换和真实导出已经可用。Video、Scene 预渲染、ADB Auto 和桌面 GUI 仍待后续阶段；这些尚无执行后端的类型在非 `--dry-run` 导出时会明确返回 `backend_unavailable`，不会生成伪造或不完整的输出。

> 仓库名 `pkg2pkgm` 对应 CLI / crate 名 `pkg2mpkg`（package → mobile package）。

## 证据边界

- Windows 行为从本地 Wallpaper Engine **2.8.26** 安装树提取，不依赖残缺第三方分析作为实现来源。
- Android 目标是官方 `io.wallpaperengine.weclient` **2.8.8**。
- 官方运行时、APK、Wine 与专有 Workshop 样例**仅作本地预言机**，不提交进本仓库。见 [reference/README.md](reference/README.md)。

## 当前能力

- 检查项目目录、`project.json`、可定位项目清单的 `.pkg`，以及直接 MP4/WebM 文件。
- 支持 Scene 和 Video；显式或推断出的 Web/Application 均返回退出码 `3`。
- 保留未知 `project.json` 字段，不修改源项目。
- 复刻 Windows 2.8.26 的 High、Balanced、Performance Scene 配置矩阵。
- 按 Windows 阈值识别 Pixel Art、Normal 和 UHD：Scene 面积小于 `307200` 时为 Pixel Art；分辨率标签面积大于 `2075520` 时优先 UHD。
- Rust 库支持任意手动目标尺寸的 Cover 和 KeepAspect 几何计算。H.264 输出要求正偶数宽高，输入素材允许奇数尺寸。
- 安全**读取**任意 `PKGM` + 4 位版本（含第三方/旧版 **PKGM0014**–**PKGM0020** 等）；拒绝路径穿越、重复、越界、重叠、无效 UTF-8 和截断目录。
- `inspect` / `verify` 可直接作用于 `.mpkg`（video/scene）；Web/Application 仍拒绝（退出码 3）。
- 确定性**写入**仅 PKGM0018/PKGM0020（导出默认 0020），输出小于 4 GiB；同目录 partial、自校验与原子持久化。
- `export --dry-run` 输出稳定的 `ExportPlan` JSON，并列出后续执行需要的辅助程序能力。

## 安装

### 预编译安装包（推荐）

GitHub Actions 会为每个 release 构建各平台可直接运行的二进制包：

| 平台 | 架构 | 资源名模式 |
|---|---|---|
| Linux | x86_64 / aarch64 | `pkg2mpkg-v*-x86_64-unknown-linux-gnu.tar.gz` 等 |
| macOS | Apple Silicon / Intel | `…-aarch64-apple-darwin.tar.gz` / `…-x86_64-apple-darwin.tar.gz` |
| Windows | x86_64 | `…-x86_64-pc-windows-msvc.zip` |

发布页：https://github.com/anpplex/pkg2pkgm/releases

**macOS / Linux 一键安装（装到 `/usr/local/bin`）：**

```bash
curl -fsSL https://raw.githubusercontent.com/anpplex/pkg2pkgm/main/scripts/install.sh | bash
# 或指定版本 / 安装目录
# VERSION=v0.1.0 INSTALL_DIR=~/.local/bin bash <(curl -fsSL …/scripts/install.sh)
```

**手动解压运行：**

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

### 从源码构建

环境要求：Rust **1.97.0**（仓库 `rust-toolchain.toml` 会自动选择）。

```bash
cargo build --release -p pkg2mpkg --bin pkg2mpkg
./target/release/pkg2mpkg --help
```

检查 Scene 工程：

```bash
cargo run -p pkg2mpkg -- inspect /path/to/scene_project --json
```

生成动态 Scene 的只读导出计划：

```bash
cargo run -p pkg2mpkg -- export /path/to/scene \
  --output /tmp/scene.mpkg \
  --profile balanced \
  --dry-run \
  --json
```

校验已有官方或自生成 MPKG：

```bash
cargo run -p pkg2mpkg -- verify /path/to/wallpaper.mpkg --json
```

Scene 必须显式选择 `high`、`balanced` 或 `performance`。Video 在本阶段会保守地计划 H.264 转码；只有未来探测器证明容器、编码、像素格式、旋转、尺寸、FPS 和音频全部兼容后，才允许 passthrough。

## 导出松散 Scene

对**松散**动态 Scene 工程目录执行真实导出（非 `--dry-run`）时，需要 Wallpaper Engine 2.8.26 运行时；在非 Windows 主机上还需要 Wine：

```bash
# macOS / Linux
cargo run -p pkg2mpkg -- export /path/to/loose_scene \
  --output /tmp/scene.mpkg \
  --profile high \
  --we-runtime "/path/to/Wallpaper Engine.2.8.26" \
  --wine /path/to/wine \
  --replace \
  --json

# Windows：省略 --wine / --winepath，直接本机启动 resourcecompiler64.exe
cargo run -p pkg2mpkg -- export C:\path\to\loose_scene \
  --output C:\temp\scene.mpkg \
  --profile high \
  --we-runtime "C:\path\to\Wallpaper Engine.2.8.26" \
  --replace
```

### zcompat 兼容着色器（随 `--we-runtime` 自动）

当传入 `--we-runtime` 且运行时目录下存在官方树 `assets/zcompat/scene/shaders/` 时，动态导出会**自动**挂载兼容着色器覆盖策略（按 workshop 工程 id / `maximumprojectid` 匹配并替换命名的 frag/vert）。目录不存在时静默跳过，无需额外 CLI 开关。

### Wine / winepath 注意（macOS / Linux）

- `--wine` 在非 Windows 上为动态 Scene 导出所必需；`--winepath` 可选，默认取 `WE_WINE` 同级的 `winepath`。
- **Wine Stable 的 `winepath` 通常是指向 `wine` 的符号链接**，并通过 **argv0 基名** 选择 winepath 人格。若把该链接 `canonicalize` 掉，会变成 `wine -w …` 并以 ShellExecuteEx 失败。CLI 会保留绝对路径中的最终路径分量（不解析掉 `winepath` 这个名字）；自定义 `--winepath` 时也请指向仍叫 `winepath` 的路径。
- Wine 前缀、临时路径与 helper 调用是**任务内临时状态**，不写入输出 MPKG。

官方 Android 2.8.8 实机验收不属于 hermetic 默认质量门，但在宣称 Android 行为兼容或完整复刻前是必需资格门。见 [本地参考说明](reference/README.md)。

## Packaged scene.pkg

真实 Workshop Scene 有两种入口形态：`project.json` 直接声明 `file: scene.pkg`；或仍声明 `file: scene.json`，但松散 JSON 不存在、同路径同 stem 的 `scene.pkg` 存在。后者会保留 manifest 的逻辑入口，同时把实际输入解析为该精确 sibling `scene.pkg`；若松散 JSON 存在则始终优先使用。Android 导出前必须先解出松散 Scene 树；**不得**把原始 `.pkg` 当作 Android 载荷打进 MPKG。

### 当前状态

| 层级 | 状态 |
|---|---|
| 检测 | `inspect` 以解析后的物理入口判断是否解包：直接 `*.pkg`，以及缺失 `scene.json` 时解析出的精确 sibling `scene.pkg`，都会标记为需解包 |
| 边界 API | `ScenePackageUnpackBackend`、`unpack_scene_package_checked`、`prepare_packaged_scene_source` |
| 原生实现 | `DesktopPackageArchive` + `NativeScenePackageUnpackBackend`（core 内，**无 Wine**） |
| 容器 | 桌面 `PKGV****` 与 Android `PKGM*` **表布局同构**；仅 8 字节 magic 不同 |
| 执行管线 | 解到任务私有 `unpacked/` 后 inventory / TEX 转换 |
| CLI | 动态 Scene 导出会注入 `NativeScenePackageUnpackBackend`；松散工程无解包时为 no-op |
| 安全 | 路径规范、条目/字节上限对齐 MPKG；**禁止**把未解包 `scene.pkg` 写入 Android MPKG |

库集成方可注入自定义 `ScenePackageUnpackBackend`；不注入且源为打包工程时返回 `BackendUnavailable(scene_pkg_unpack)`（退出码 `5`）。

### CLI 导出打包工程

与松散 Scene 相同：

```bash
pkg2mpkg export /path/to/workshop-project \
  --output out.mpkg \
  --profile high \
  --we-runtime '/path/to/Wallpaper Engine.2.8.26' \
  --wine /path/to/wine --winepath /path/to/winepath   # 非 Windows
```

执行链为 `scene.pkg` → 松散 Scene 树 → 转换 `.tex` → PKGM0020。

## CLI 退出码

| 退出码 | 含义 |
|---:|---|
| `0` | 成功 |
| `2` | 参数或计划无效 |
| `3` | Web/Application 不支持移动导出 |
| `4` | 项目或 MPKG 无效 |
| `5` | 所需导出后端尚不可用 |
| `6` | 转换失败 |
| `7` | 输出 I/O 或 4 GiB 限制 |
| `8` | 输出验证失败 |
| `9` | 设备操作失败 |
| `130` | 任务取消 |

命令带 `--json` 时，业务错误写入 stderr：

```json
{
  "code": "unsupported_wallpaper_type",
  "stage": "inspect",
  "message": "unsupported wallpaper type: web"
}
```

## 测试

完整质量门：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

默认测试套件是 **hermetic** 的：不依赖 Wine、官方 resourcecompiler、APK 或任何专有资产。

### 本地 Dino 动态 Scene 预言机（默认 `#[ignore]`）

在已安装 Windows WE 2.8.26 研究树、Wine（macOS/Linux）以及可选的官方 Android Dino MPKG 时：

```bash
WE_RUNTIME='/path/to/Wallpaper Engine.2.8.26' \
WE_DINO_PROJECT="$WE_RUNTIME/projects/defaultprojects/dino_run" \
WE_WINE=/path/to/wine \
WE_DINO_MPKG=/tmp/dino_run.mpkg \
  cargo test -p pkg2mpkg-core --test dino_dynamic_reference -- --ignored --nocapture
```

| 环境变量 | 用途 |
|---|---|
| `WE_RUNTIME` | WE 2.8.26 安装根目录（含 `distribution/bin/resourcecompiler64.exe`） |
| `WE_DINO_PROJECT` | 松散 Dino Scene 工程目录 |
| `WE_WINE` | Wine 可执行文件（非 Windows 主机必填） |
| `WE_WINEPATH` | 可选；默认使用 `WE_WINE` 同级的 `winepath` |
| `WE_DINO_MPKG` | 可选；官方 Android Dino MPKG，仅做结构对照 |

完整配方见 [reference/README.md](reference/README.md)。

### 官方 Android Dino MPKG 只读参考

```bash
WE_DINO_MPKG=/tmp/dino_run.mpkg \
  cargo test -p pkg2mpkg-fixtures --test reference_dino -- --ignored
```

## 仓库结构

```
.
├── Cargo.toml              # workspace
├── crates/
│   ├── cli/                # pkg2mpkg 可执行文件
│   ├── core/               # 检查 / 计划 / MPKG / Scene 管线
│   ├── codecs/             # resourcecompiler + Wine 适配
│   └── fixtures/           # 测试固件与参考读取
├── reference/              # 本地预言机说明（不含二进制）
├── README.md               # 中文（默认）
└── README.en.md            # English
```

## 后续阶段

1. Video 与 Scene 预渲染：FFmpeg、任意分辨率/FPS/裁切、SceneCaptureBackend、Windows 时长差分。
2. ADB 与桌面 GUI：设备自动分辨率、传输验收、egui 工作流、三平台发行包。

在动态 Scene 通过官方 Android 2.8.8 实机验收、且 Video/GUI 阶段完成前，项目不会宣称完整复刻 `exportMobilePkg`。

## 许可

MIT
