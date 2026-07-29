use std::{fs, path::Path};

pub struct SyntheticProject {
    dir: tempfile::TempDir,
    entry: &'static str,
}

impl SyntheticProject {
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn entry_path(&self) -> std::path::PathBuf {
        self.dir.path().join(self.entry)
    }
}

pub fn synthetic_scene_project() -> SyntheticProject {
    let dir = tempfile::tempdir().expect("create synthetic Scene directory");
    fs::write(
        dir.path().join("scene.json"),
        br#"{"general":{"orthogonalprojection":{"width":343,"height":193}},"objects":[]}"#,
    )
    .expect("write synthetic scene.json");
    fs::write(
        dir.path().join("project.json"),
        br#"{"title":"Synthetic Scene","type":"scene","file":"scene.json","tags":["Pixel art"],"vendor":{"x":7}}"#,
    )
    .expect("write synthetic Scene project.json");
    SyntheticProject {
        dir,
        entry: "scene.json",
    }
}

pub fn synthetic_video_project() -> SyntheticProject {
    let dir = tempfile::tempdir().expect("create synthetic Video directory");
    fs::write(dir.path().join("clip.mp4"), b"synthetic video bytes")
        .expect("write synthetic clip.mp4");
    fs::write(
        dir.path().join("project.json"),
        br#"{"title":"Synthetic Video","type":"video","file":"clip.mp4"}"#,
    )
    .expect("write synthetic Video project.json");
    SyntheticProject {
        dir,
        entry: "clip.mp4",
    }
}

pub fn synthetic_web_project() -> SyntheticProject {
    let dir = tempfile::tempdir().expect("create synthetic Web directory");
    fs::write(dir.path().join("index.html"), b"<p>synthetic web</p>")
        .expect("write synthetic index.html");
    fs::write(
        dir.path().join("project.json"),
        br#"{"title":"Synthetic Web","type":"web","file":"index.html"}"#,
    )
    .expect("write synthetic Web project.json");
    SyntheticProject {
        dir,
        entry: "index.html",
    }
}

pub fn synthetic_application_project() -> SyntheticProject {
    let dir = tempfile::tempdir().expect("create synthetic Application directory");
    fs::write(dir.path().join("demo.exe"), b"synthetic application bytes")
        .expect("write synthetic demo.exe");
    fs::write(
        dir.path().join("project.json"),
        br#"{"title":"Synthetic Application","type":"application","file":"demo.exe"}"#,
    )
    .expect("write synthetic Application project.json");
    SyntheticProject {
        dir,
        entry: "demo.exe",
    }
}
