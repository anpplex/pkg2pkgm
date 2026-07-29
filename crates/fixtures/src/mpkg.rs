/// Build a synthetic package with an arbitrary 8-byte magic (PKGM / PKGV / …).
///
/// Layout matches RePKG / `pkg2mpkg.py` `write_pkg`: length-prefixed magic,
/// entry table, then contiguous payload blobs.
pub fn raw_mpkg(version: &str, entries: &[(&str, &[u8])]) -> Vec<u8> {
    assert_eq!(version.len(), 8, "package magic must be exactly 8 bytes");
    let mut table = Vec::new();
    let mut payload = Vec::new();
    for (path, bytes) in entries {
        let path_len = u32::try_from(path.len()).expect("fixture path fits u32");
        let offset = u32::try_from(payload.len()).expect("fixture payload offset fits u32");
        let size = u32::try_from(bytes.len()).expect("fixture payload size fits u32");
        table.extend_from_slice(&path_len.to_le_bytes());
        table.extend_from_slice(path.as_bytes());
        table.extend_from_slice(&offset.to_le_bytes());
        table.extend_from_slice(&size.to_le_bytes());
        payload.extend_from_slice(bytes);
    }
    let count = u32::try_from(entries.len()).expect("fixture entry count fits u32");
    let mut out = Vec::new();
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(version.as_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&table);
    out.extend_from_slice(&payload);
    out
}

/// Synthetic desktop `scene.pkg` (typically `PKGV0001` / `PKGV0005`).
///
/// Same binary layout as [`raw_mpkg`]; use for native PKGV unpack tests.
pub fn raw_pkg(version: &str, entries: &[(&str, &[u8])]) -> Vec<u8> {
    raw_mpkg(version, entries)
}
