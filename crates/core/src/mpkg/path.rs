use crate::{Error, Result};

/// Pure archive-path normalization shared by MPKG IO and Scene inventory.
///
/// Rejects empty paths, NULs, backslashes, absolute/root forms, Windows drive
/// prefixes, and empty / `.` / `..` components. On success returns the input
/// owned (paths are already normalized forward-slash relative strings).
pub(crate) fn normalize_archive_path(path: &str) -> std::result::Result<String, String> {
    let bytes = path.as_bytes();
    let drive_prefix = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if path.is_empty()
        || path.contains('\0')
        || path.contains('\\')
        || path.starts_with('/')
        || drive_prefix
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(path.to_owned());
    }
    Ok(path.to_owned())
}

pub(crate) fn validate_archive_path(path: &str) -> Result<String> {
    // Writer / MPKG path gate — keeps InvalidMpkg for mobile archives.
    normalize_archive_path(path)
        .map_err(|path| Error::invalid_mpkg(format!("unsafe archive path: {path:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_rejects_escape_forms() {
        for path in ["", "/a", "a\\b", "a/../b", "a//b", "./a", "C:foo", "c:bar"] {
            assert!(normalize_archive_path(path).is_err(), "{path}");
        }
    }

    #[test]
    fn normalize_accepts_relative_utf8() {
        assert_eq!(
            normalize_archive_path("materials/opaque.tex").unwrap(),
            "materials/opaque.tex"
        );
    }
}
