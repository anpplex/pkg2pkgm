mod desktop;
pub(crate) mod path;
mod reader;
mod writer;

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub use desktop::{DesktopPackageArchive, NativeScenePackageUnpackBackend};
pub use reader::{MpkgArchive, MpkgEntry};
pub use writer::{MpkgBuilder, OverwritePolicy, WriteReport};

/// Android MPKG container version identified by the 8-byte `PKGM####` magic.
///
/// Known variants map to named cases; any other `PKGM` + four ASCII digits is
/// accepted for **read-only** open/verify/inspect via [`Self::OtherPkgm`].
/// The writer only emits [`Self::Pkgm0018`] and [`Self::Pkgm0020`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContainerVersion {
    Pkgm0014,
    Pkgm0018,
    Pkgm0020,
    /// Other `PKGM####` recognized for read-only open (digits are ASCII `'0'`–`'9'`).
    OtherPkgm {
        digits: [u8; 4],
    },
}

impl ContainerVersion {
    /// True for versions the export writer is allowed to emit.
    pub const fn is_writable(self) -> bool {
        matches!(self, Self::Pkgm0018 | Self::Pkgm0020)
    }

    /// 8-byte magic string (`PKGM` + four digits).
    pub fn as_magic(self) -> Cow<'static, str> {
        match self {
            Self::Pkgm0014 => Cow::Borrowed("PKGM0014"),
            Self::Pkgm0018 => Cow::Borrowed("PKGM0018"),
            Self::Pkgm0020 => Cow::Borrowed("PKGM0020"),
            Self::OtherPkgm { digits } => {
                let mut magic = String::with_capacity(8);
                magic.push_str("PKGM");
                magic.push(char::from(digits[0]));
                magic.push(char::from(digits[1]));
                magic.push(char::from(digits[2]));
                magic.push(char::from(digits[3]));
                Cow::Owned(magic)
            }
        }
    }

    pub(crate) fn from_magic(magic: &[u8]) -> Result<Self> {
        if magic.len() != 8 {
            return Err(Error::invalid_mpkg("unsupported container magic"));
        }
        if magic[..4] != *b"PKGM" {
            return Err(Error::invalid_mpkg("unsupported container magic"));
        }
        let digits = [magic[4], magic[5], magic[6], magic[7]];
        if !digits.iter().all(u8::is_ascii_digit) {
            return Err(Error::invalid_mpkg("unsupported container magic"));
        }
        match &digits {
            b"0014" => Ok(Self::Pkgm0014),
            b"0018" => Ok(Self::Pkgm0018),
            b"0020" => Ok(Self::Pkgm0020),
            _ => Ok(Self::OtherPkgm { digits }),
        }
    }
}
