use std::{
    collections::{BTreeSet, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{Error, Result, Stage, mpkg::path::normalize_archive_path};

/// Read-only closure of local Scene references discovered from the declared
/// entry, transitive local JSON string paths, and explicit
/// `engine.registerAsset(...)` script calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneReferenceReport {
    pub scene_entry: String,
    /// Existing local project-relative paths, sorted by archive path bytes.
    pub local_references: Vec<String>,
}

/// Validate Scene references without mutating project files or rewriting scripts.
pub fn validate_scene_references(
    project_root: &Path,
    scene_entry: &str,
) -> Result<SceneReferenceReport> {
    let scene_entry =
        normalize_archive_path(scene_entry).map_err(|path| Error::InvalidProject {
            reason: format!("unsafe scene entry path: {path:?}"),
        })?;

    ensure_regular_file(project_root, &scene_entry)?.ok_or_else(|| Error::InvalidProject {
        reason: format!("scene entry is missing: {scene_entry}"),
    })?;

    let mut local_references = BTreeSet::new();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();

    local_references.insert(scene_entry.clone());
    queue.push_back(scene_entry.clone());

    while let Some(archive_path) = queue.pop_front() {
        if !visited.insert(archive_path.clone()) {
            continue;
        }

        if is_json_path(&archive_path) {
            let bytes = read_file(project_root, &archive_path)?;
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|source| Error::InvalidProject {
                    reason: format!("referenced JSON is invalid ({archive_path}): {source}"),
                })?;
            for candidate in collect_json_strings(&value) {
                process_json_path_candidate(
                    project_root,
                    &candidate,
                    &mut local_references,
                    &mut queue,
                )?;
            }
        } else if is_script_path(&archive_path) {
            let bytes = read_file(project_root, &archive_path)?;
            for asset in extract_register_assets(&bytes)? {
                process_register_asset(project_root, &asset, &mut local_references, &mut queue)?;
            }
        }
    }

    Ok(SceneReferenceReport {
        scene_entry,
        local_references: local_references.into_iter().collect(),
    })
}

fn process_json_path_candidate(
    project_root: &Path,
    candidate: &str,
    local_references: &mut BTreeSet<String>,
    queue: &mut VecDeque<String>,
) -> Result<()> {
    if !looks_like_relative_path(candidate) {
        return Ok(());
    }
    let path = normalize_archive_path(candidate).map_err(|path| Error::InvalidProject {
        reason: format!("unsafe scene reference path: {path:?}"),
    })?;
    match ensure_regular_file(project_root, &path)? {
        Some(_) => {
            if local_references.insert(path.clone()) {
                queue.push_back(path);
            }
            Ok(())
        }
        // Missing local file: treat as project-external WE global / optional.
        None => Ok(()),
    }
}

fn process_register_asset(
    project_root: &Path,
    asset: &str,
    local_references: &mut BTreeSet<String>,
    queue: &mut VecDeque<String>,
) -> Result<()> {
    let path = normalize_archive_path(asset).map_err(|path| Error::InvalidProject {
        reason: format!("unsafe engine.registerAsset path: {path:?}"),
    })?;
    match ensure_regular_file(project_root, &path)? {
        Some(_) => {
            if local_references.insert(path.clone()) {
                queue.push_back(path);
            }
            Ok(())
        }
        None => Err(Error::InvalidProject {
            reason: format!("engine.registerAsset reference is missing: {path}"),
        }),
    }
}

fn collect_json_strings(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    collect_json_strings_into(value, &mut out);
    out
}

fn collect_json_strings_into(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) => out.push(text.clone()),
        Value::Array(items) => {
            for item in items {
                collect_json_strings_into(item, out);
            }
        }
        Value::Object(map) => {
            for child in map.values() {
                collect_json_strings_into(child, out);
            }
        }
        _ => {}
    }
}

fn looks_like_relative_path(value: &str) -> bool {
    if value.is_empty() || value.contains('\0') || value.contains(' ') {
        return false;
    }
    if value.contains('/') {
        return true;
    }
    // Bare asset-like names such as `scene.json` / `opaque.tex`.
    let Some((_, ext)) = value.rsplit_once('.') else {
        return false;
    };
    !ext.is_empty()
        && ext.len() <= 16
        && ext
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn is_json_path(path: &str) -> bool {
    has_ascii_extension(path, "json")
}

fn is_script_path(path: &str) -> bool {
    has_ascii_extension(path, "js")
}

fn has_ascii_extension(path: &str, expected: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

fn ensure_regular_file(project_root: &Path, archive_path: &str) -> Result<Option<PathBuf>> {
    let full = project_root.join(archive_path);
    let metadata = match fs::symlink_metadata(&full) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(Error::Io {
                stage: Stage::Inspect,
                path: full,
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(Error::InvalidProject {
            reason: format!("scene reference must not be a symlink: {archive_path}"),
        });
    }
    if metadata.is_file() {
        Ok(Some(full))
    } else {
        Ok(None)
    }
}

fn read_file(project_root: &Path, archive_path: &str) -> Result<Vec<u8>> {
    let full = project_root.join(archive_path);
    fs::read(&full).map_err(|source| Error::Io {
        stage: Stage::Inspect,
        path: full,
        source,
    })
}

/// Small lexical scanner for `engine.registerAsset('...')` / `"..."` calls.
///
/// Ignores line/block comments, ordinary strings, template strings, and
/// identifier lookalikes such as `fakeengine.registerAsset(...)`.
fn extract_register_assets(source: &[u8]) -> Result<Vec<String>> {
    let mut assets = Vec::new();
    let mut i = 0usize;
    while i < source.len() {
        match source[i] {
            b'/' if source.get(i + 1) == Some(&b'/') => {
                i += 2;
                while i < source.len() && source[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if source.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < source.len() && !(source[i] == b'*' && source[i + 1] == b'/') {
                    i += 1;
                }
                i = i.saturating_add(2);
            }
            b'\'' | b'"' => {
                i = skip_quoted(source, i)?;
            }
            b'`' => {
                i = skip_template(source, i)?;
            }
            b'e' if match_keyword_at(source, i, b"engine") => {
                let after_engine = i + b"engine".len();
                let after_dot = skip_ws(source, after_engine);
                if source.get(after_dot) != Some(&b'.') {
                    i += 1;
                    continue;
                }
                let after_ident = skip_ws(source, after_dot + 1);
                if !match_keyword_at(source, after_ident, b"registerAsset") {
                    i += 1;
                    continue;
                }
                let after_name = after_ident + b"registerAsset".len();
                let after_paren = skip_ws(source, after_name);
                if source.get(after_paren) != Some(&b'(') {
                    i += 1;
                    continue;
                }
                let after_open = skip_ws(source, after_paren + 1);
                let Some(quote) = source.get(after_open).copied() else {
                    return Err(Error::InvalidProject {
                        reason: "engine.registerAsset call is truncated".into(),
                    });
                };
                if quote != b'\'' && quote != b'"' {
                    // Dynamic / non-literal argument: not an explicit mandatory ref.
                    i = after_open;
                    continue;
                }
                let (path, next) = read_quoted(source, after_open)?;
                assets.push(path);
                i = next;
            }
            _ => i += 1,
        }
    }
    Ok(assets)
}

fn match_keyword_at(source: &[u8], index: usize, keyword: &[u8]) -> bool {
    if index + keyword.len() > source.len() {
        return false;
    }
    if !source[index..index + keyword.len()].eq_ignore_ascii_case(keyword) {
        return false;
    }
    // Left boundary: previous byte must not be an identifier character.
    if index > 0 && is_ident_byte(source[index - 1]) {
        return false;
    }
    // Right boundary: next byte must not continue the identifier.
    let end = index + keyword.len();
    if end < source.len() && is_ident_byte(source[end]) {
        return false;
    }
    true
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

fn skip_ws(source: &[u8], mut index: usize) -> usize {
    while index < source.len() && source[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

fn skip_quoted(source: &[u8], start: usize) -> Result<usize> {
    let (_, end) = read_quoted(source, start)?;
    Ok(end)
}

fn read_quoted(source: &[u8], start: usize) -> Result<(String, usize)> {
    let quote = source[start];
    let mut i = start + 1;
    let mut out = Vec::new();
    while i < source.len() {
        let byte = source[i];
        if byte == quote {
            let text = String::from_utf8(out).map_err(|_| Error::InvalidProject {
                reason: "engine.registerAsset path is not valid UTF-8".into(),
            })?;
            return Ok((text, i + 1));
        }
        if byte == b'\\' {
            i += 1;
            if i >= source.len() {
                break;
            }
            // Preserve common JS single-character escapes as the escaped byte.
            out.push(source[i]);
            i += 1;
            continue;
        }
        if byte == b'\n' || byte == b'\r' {
            return Err(Error::InvalidProject {
                reason: "unterminated string while scanning engine.registerAsset".into(),
            });
        }
        out.push(byte);
        i += 1;
    }
    Err(Error::InvalidProject {
        reason: "unterminated string while scanning engine.registerAsset".into(),
    })
}

fn skip_template(source: &[u8], start: usize) -> Result<usize> {
    let mut i = start + 1;
    while i < source.len() {
        match source[i] {
            b'`' => return Ok(i + 1),
            b'\\' => {
                i = i.saturating_add(2);
            }
            b'$' if source.get(i + 1) == Some(&b'{') => {
                i += 2;
                let mut depth = 1usize;
                while i < source.len() && depth > 0 {
                    match source[i] {
                        b'\'' | b'"' => i = skip_quoted(source, i)?,
                        b'`' => i = skip_template(source, i)?,
                        b'/' if source.get(i + 1) == Some(&b'/') => {
                            i += 2;
                            while i < source.len() && source[i] != b'\n' {
                                i += 1;
                            }
                        }
                        b'/' if source.get(i + 1) == Some(&b'*') => {
                            i += 2;
                            while i + 1 < source.len()
                                && !(source[i] == b'*' && source[i + 1] == b'/')
                            {
                                i += 1;
                            }
                            i = i.saturating_add(2);
                        }
                        b'{' => {
                            depth += 1;
                            i += 1;
                        }
                        b'}' => {
                            depth -= 1;
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
            }
            _ => i += 1,
        }
    }
    Err(Error::InvalidProject {
        reason: "unterminated template string while scanning scripts".into(),
    })
}
