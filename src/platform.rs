//! OS-specific file metadata, so the index logic stays free of `cfg`s.
//!
//! - [`stat`]: the [`Stat`] the index records for an entry.
//! - [`file_id`]: `(dev, ino)` of an open file, to detect a file being
//!   replaced between stat and hashing.
//! - [`special_kind`]: a name for entries that are neither file, directory
//!   nor symlink.
//! - [`sync_dir`]: makes a rename in a directory durable.

pub use imp::{file_id, special_kind, stat, sync_dir};

#[cfg(unix)]
mod imp {
    use crate::index::Stat;
    use std::fs::{File, FileType, Metadata};
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    use std::path::Path;

    /// Never fails on Unix; everything comes from `meta`.
    pub fn stat(_path: &Path, meta: &Metadata) -> std::io::Result<Stat> {
        Ok(Stat {
            size: meta.size(),
            mtime: [meta.mtime(), meta.mtime_nsec()],
            ctime: [meta.ctime(), meta.ctime_nsec()],
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }

    pub fn file_id(file: &File) -> std::io::Result<(u64, u64)> {
        let meta = file.metadata()?;
        Ok((meta.dev(), meta.ino()))
    }

    pub fn special_kind(t: FileType) -> &'static str {
        if t.is_fifo() {
            "fifo"
        } else if t.is_socket() {
            "socket"
        } else if t.is_block_device() {
            "block_device"
        } else if t.is_char_device() {
            "char_device"
        } else {
            "unknown"
        }
    }

    /// fsyncs the directory, so a rename inside it survives a crash.
    pub fn sync_dir(dir: &Path) -> std::io::Result<()> {
        File::open(dir)?.sync_all()
    }
}

#[cfg(windows)]
mod imp {
    use crate::index::Stat;
    use std::fs::{File, FileType, Metadata, OpenOptions};
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FileBasicInfo, GetFileInformationByHandle,
        GetFileInformationByHandleEx,
    };

    /// 100 ns intervals between 1601-01-01 (FILETIME epoch) and 1970-01-01.
    const UNIX_EPOCH_AS_FILETIME: i64 = 116_444_736_000_000_000;

    /// Windows keeps the file ID and the change time (the equivalent of Unix
    /// ctime) out of `Metadata`, so this opens a handle to the entry itself
    /// (not following symlinks, directories allowed) with attribute-only
    /// access and full sharing, so it neither needs read permission nor
    /// blocks other processes.
    pub fn stat(path: &Path, meta: &Metadata) -> std::io::Result<Stat> {
        let handle = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let basic = basic_info(&handle)?;
        let (dev, ino) = file_id(&handle)?;
        Ok(Stat {
            size: meta.len(),
            mtime: unix_time(basic.LastWriteTime),
            ctime: unix_time(basic.ChangeTime),
            dev,
            ino,
        })
    }

    /// Volume serial number and 64-bit file index. On ReFS file IDs are
    /// 128 bits and the 64-bit index is not guaranteed unique, so there a
    /// replaced file could in rare cases go unnoticed by the ID check alone.
    pub fn file_id(file: &File) -> std::io::Result<(u64, u64)> {
        // SAFETY: plain C struct, all-zero is a valid value.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the handle is valid for the lifetime of `file`, and `info`
        // is a properly sized out-parameter.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let ino = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
        Ok((u64::from(info.dwVolumeSerialNumber), ino))
    }

    fn basic_info(file: &File) -> std::io::Result<FILE_BASIC_INFO> {
        // SAFETY: plain C struct, all-zero is a valid value.
        let mut info: FILE_BASIC_INFO = unsafe { std::mem::zeroed() };
        // SAFETY: the handle is valid for the lifetime of `file`, and the
        // buffer is a FILE_BASIC_INFO of the size passed, as FileBasicInfo
        // requires.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileBasicInfo,
                (&raw mut info).cast(),
                size_of::<FILE_BASIC_INFO>() as u32,
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(info)
    }

    /// FILETIME (100 ns since 1601) as `[secs, nsecs]` since the Unix epoch.
    fn unix_time(filetime: i64) -> [i64; 2] {
        let t = filetime - UNIX_EPOCH_AS_FILETIME;
        [t.div_euclid(10_000_000), t.rem_euclid(10_000_000) * 100]
    }

    /// Windows has no fifos, sockets or device files in the file system that
    /// std can tell apart; anything that gets here is some other reparse
    /// point.
    pub fn special_kind(_t: FileType) -> &'static str {
        "unknown"
    }

    /// No-op: directories can't be fsynced on Windows, and NTFS journals
    /// the rename's metadata change itself.
    pub fn sync_dir(_dir: &Path) -> std::io::Result<()> {
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn filetime_conversion() {
            assert_eq!(unix_time(UNIX_EPOCH_AS_FILETIME), [0, 0]);
            assert_eq!(unix_time(UNIX_EPOCH_AS_FILETIME + 15), [0, 1500]);
            assert_eq!(unix_time(UNIX_EPOCH_AS_FILETIME - 1), [-1, 999_999_900]);
        }
    }
}