//! Formatting helpers shared by the report renderers. The results are not
//! escaped: each renderer escapes them for its own output format.

use crate::journal::Digest;

/// The digest as shown in reports: the prefixed hash, the symlink target or
/// the reason the file could not be hashed.
pub(super) fn digest(digest: &Digest) -> String {
    match digest {
        Digest::Hash(hash) => hash.clone(),
        Digest::Symlink(target) => format!("symlink to {}", target.display()),
        Digest::Error(msg) => format!("not hashed: {}", msg),
    }
}

/// Formats `ts` in the local time of the host that wrote the entry. Falls
/// back to UTC if the stored offset is out of range.
pub(super) fn local_time(ts: jiff::Timestamp, offset_seconds: i32) -> String {
    match jiff::tz::Offset::from_seconds(offset_seconds) {
        Ok(offset) => ts
            .to_zoned(jiff::tz::TimeZone::fixed(offset))
            .strftime("%Y-%m-%d %H:%M:%S %:z")
            .to_string(),
        Err(_) => ts.to_string(),
    }
}
