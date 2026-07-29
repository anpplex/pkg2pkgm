use std::fs;

use pkg2mpkg_core::{ErrorCode, MpkgArchive};
use tempfile::tempdir;

struct RawEntry<'a> {
    path: &'a [u8],
    offset: u32,
    size: u32,
}

fn custom_raw(magic: &[u8], count: u32, entries: &[RawEntry<'_>], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(magic.len() as u32).to_le_bytes());
    out.extend_from_slice(magic);
    out.extend_from_slice(&count.to_le_bytes());
    for entry in entries {
        out.extend_from_slice(&(entry.path.len() as u32).to_le_bytes());
        out.extend_from_slice(entry.path);
        out.extend_from_slice(&entry.offset.to_le_bytes());
        out.extend_from_slice(&entry.size.to_le_bytes());
    }
    out.extend_from_slice(payload);
    out
}

fn malformed_cases() -> Vec<(&'static str, Vec<u8>)> {
    let one = |path: &'static [u8], offset, size, payload: &'static [u8]| {
        custom_raw(b"PKGM0020", 1, &[RawEntry { path, offset, size }], payload)
    };
    vec![
        ("parent", one(b"../x", 0, 1, b"x")),
        ("absolute", one(b"/x", 0, 1, b"x")),
        ("drive", one(b"C:/x", 0, 1, b"x")),
        ("backslash", one(br"a\b", 0, 1, b"x")),
        ("nul", one(b"a\0b", 0, 1, b"x")),
        ("empty_component", one(b"a//b", 0, 1, b"x")),
        ("dot_component", one(b"a/./b", 0, 1, b"x")),
        ("invalid_utf8", one(&[0xff], 0, 1, b"x")),
        (
            "duplicate",
            custom_raw(
                b"PKGM0020",
                2,
                &[
                    RawEntry {
                        path: b"a",
                        offset: 0,
                        size: 1,
                    },
                    RawEntry {
                        path: b"a",
                        offset: 1,
                        size: 1,
                    },
                ],
                b"xy",
            ),
        ),
        ("offset", one(b"a", 2, 1, b"x")),
        ("size", one(b"a", 0, 2, b"x")),
        (
            "overlap",
            custom_raw(
                b"PKGM0020",
                2,
                &[
                    RawEntry {
                        path: b"a",
                        offset: 0,
                        size: 2,
                    },
                    RawEntry {
                        path: b"b",
                        offset: 1,
                        size: 2,
                    },
                ],
                b"xyz",
            ),
        ),
        ("magic_length", custom_raw(b"PKGM020", 0, &[], b"")),
        // Non-PKGM or non-digit suffix is rejected; PKGM#### (any digits) is accepted on open.
        ("unknown_magic", custom_raw(b"PKGX0014", 0, &[], b"")),
        ("non_digit_magic", custom_raw(b"PKGM00AB", 0, &[], b"")),
        ("entry_count", custom_raw(b"PKGM0020", 1_000_001, &[], b"")),
        ("truncated_header", vec![8, 0, 0]),
    ]
}

#[test]
fn rejects_malformed_archives() {
    let dir = tempdir().unwrap();
    let mut cases = malformed_cases();
    let mut excessive_path = Vec::new();
    excessive_path.extend_from_slice(&8u32.to_le_bytes());
    excessive_path.extend_from_slice(b"PKGM0020");
    excessive_path.extend_from_slice(&1u32.to_le_bytes());
    excessive_path.extend_from_slice(&16_385u32.to_le_bytes());
    cases.push(("path_length", excessive_path));

    for (name, bytes) in cases {
        let path = dir.path().join(format!("{name}.mpkg"));
        fs::write(&path, bytes).unwrap();
        let error = MpkgArchive::open(&path).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidMpkg, "{name}");
    }
}
