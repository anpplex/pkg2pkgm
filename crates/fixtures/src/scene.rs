use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

/// Copyright-free dynamic Scene fixture used by source-inventory and export tests.
pub struct DynamicSceneProject {
    dir: tempfile::TempDir,
}

impl DynamicSceneProject {
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn entry_path(&self) -> PathBuf {
        self.dir.path().join("scene.json")
    }

    pub fn project_json_path(&self) -> PathBuf {
        self.dir.path().join("project.json")
    }

    pub fn preview_path(&self) -> PathBuf {
        self.dir.path().join("preview.jpg")
    }

    /// Install a symlink that points outside the project root (Unix only).
    #[cfg(unix)]
    pub fn install_escape_symlink(&self) -> PathBuf {
        let outside = self
            .dir
            .path()
            .parent()
            .expect("tempdir parent")
            .join(format!(
                "pkg2mpkg-fixture-secret-{}",
                self.dir
                    .path()
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("x")
            ));
        fs::write(&outside, b"should-not-be-followed").expect("write outside secret");
        let link = self.dir.path().join("escape.link");
        std::os::unix::fs::symlink(&outside, &link).expect("create escape symlink");
        link
    }
}

/// Build a small dynamic Scene tree with runtime files, nested paths, editor
/// sidecars, task debris, output/partial noise, and a retained dotfile.
///
/// Symlink escape candidates are installed on demand via
/// [`DynamicSceneProject::install_escape_symlink`] so happy-path inventory
/// tests can use the same fixture without rejecting the tree.
pub fn dynamic_scene_project() -> DynamicSceneProject {
    let dir = tempfile::tempdir().expect("create dynamic Scene fixture directory");
    let root = dir.path();

    fs::create_dir_all(root.join("materials")).expect("materials dir");
    fs::create_dir_all(root.join("shaders/effects")).expect("shaders dir");
    fs::create_dir_all(root.join("sounds")).expect("sounds dir");
    fs::create_dir_all(root.join("nested/deep")).expect("nested dir");
    fs::create_dir_all(root.join("unused")).expect("unused dir");
    fs::create_dir_all(root.join(".pkg2mpkg-debris")).expect("task debris dir");
    fs::create_dir_all(root.join(".Pkg2Mpkg-Stash")).expect("case-variant debris dir");

    fs::write(
        root.join("scene.json"),
        br#"{"general":{"orthogonalprojection":{"width":640,"height":360}},"objects":[{"name":"layer"}]}"#,
    )
    .expect("write scene.json");
    fs::write(
        root.join("project.json"),
        br#"{"title":"Dynamic Fixture","type":"scene","file":"scene.json","preview":"preview.jpg","tags":["test"],"vendor":{"fixture":true}}"#,
    )
    .expect("write project.json");
    fs::write(root.join("preview.jpg"), b"JPEG-PREVIEW-BYTES").expect("write preview.jpg");

    fs::write(
        root.join("materials/main.json"),
        br#"{"passes":[{"textures":["materials/opaque.tex"]}]}"#,
    )
    .expect("write material json");
    // Opaque TEX-like bytes; conversion treats them as helper input later.
    fs::write(
        root.join("materials/opaque.tex"),
        b"TEXV0005\0TEXI0001\0synthetic-tex-payload",
    )
    .expect("write opaque.tex");
    fs::write(
        root.join("materials/opaque.tex-json"),
        br#"{"editorOnly":true}"#,
    )
    .expect("write .tex-json sidecar");
    fs::write(
        root.join("materials/opaque.TEX-JSON"),
        br#"{"editorOnly":true,"case":"upper"}"#,
    )
    .expect("write case-variant .tex-json sidecar");

    fs::write(
        root.join("shaders/effects/pulse.frag"),
        b"// synthetic fragment shader\nvoid main() {}\n",
    )
    .expect("write shader");
    fs::write(root.join("sounds/click.wav"), b"RIFF....WAVEfmt ").expect("write sound");
    fs::write(root.join("nested/deep/note.txt"), b"nested runtime file").expect("write nested");
    fs::write(root.join("unused/extra.bin"), b"unused but retained").expect("write unused");
    fs::write(root.join(".keep-dotfile"), b"dotfile must remain").expect("write dotfile");

    fs::write(root.join(".pkg2mpkg-debris/tmp.bin"), b"task debris").expect("write debris");
    fs::write(root.join(".Pkg2Mpkg-Stash/tmp.bin"), b"case debris").expect("write case debris");
    fs::write(root.join("export.mpkg"), b"fake-mpkg").expect("write .mpkg");
    fs::write(root.join("export.MPKG"), b"fake-MPKG").expect("write case .mpkg");
    fs::write(root.join("stage.partial"), b"partial").expect("write .partial");
    fs::write(root.join("stage.PARTIAL"), b"PARTIAL").expect("write case .partial");

    DynamicSceneProject { dir }
}

/// Snapshot metadata for asserting the inventory never mutates sources.
pub fn snapshot_tree(root: &Path) -> Vec<(PathBuf, u64, Option<std::time::SystemTime>)> {
    fn walk(
        dir: &Path,
        out: &mut Vec<(PathBuf, u64, Option<std::time::SystemTime>)>,
    ) -> std::io::Result<()> {
        let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let meta = fs::symlink_metadata(&path)?;
            if meta.file_type().is_dir() && !meta.file_type().is_symlink() {
                walk(&path, out)?;
            } else {
                out.push((path, meta.len(), meta.modified().ok()));
            }
        }
        Ok(())
    }

    let mut out = Vec::new();
    walk(root, &mut out).expect("snapshot source tree");
    out
}

/// Helper used by tests that need an explicit regular file writer.
pub fn write_bytes(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    let mut file = File::create(path).expect("create file");
    file.write_all(bytes).expect("write bytes");
}
