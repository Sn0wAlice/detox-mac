//! Directory listing with attributes, in bulk.
//!
//! `getattrlistbulk(2)` returns the name, type, allocated size, inode and
//! flags of a whole batch of entries in one call. Listing a directory and
//! then `lstat`-ing each entry costs one path lookup per file on top; on a
//! home directory of a million files that was two thirds of a walk.

use std::ffi::{CString, OsString};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

/// What a walk needs to know about one entry.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: OsString,
    pub kind: Kind,
    /// Bytes allocated on disk (all forks), 0 for directories.
    pub allocated: u64,
    pub dev: u64,
    pub ino: u64,
    pub nlink: u32,
    /// `st_flags`, where `SF_DATALESS` marks an iCloud placeholder.
    pub flags: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

#[repr(C)]
struct AttrList {
    bitmapcount: u16,
    reserved: u16,
    commonattr: u32,
    volattr: u32,
    dirattr: u32,
    fileattr: u32,
    forkattr: u32,
}

const ATTR_BIT_MAP_COUNT: u16 = 5;
const ATTR_CMN_NAME: u32 = 0x0000_0001;
const ATTR_CMN_DEVID: u32 = 0x0000_0002;
const ATTR_CMN_OBJTYPE: u32 = 0x0000_0008;
const ATTR_CMN_FLAGS: u32 = 0x0004_0000;
const ATTR_CMN_FILEID: u32 = 0x0200_0000;
const ATTR_CMN_ERROR: u32 = 0x2000_0000;
const ATTR_CMN_RETURNED_ATTRS: u32 = 0x8000_0000;
const ATTR_FILE_LINKCOUNT: u32 = 0x0000_0001;
const ATTR_FILE_ALLOCSIZE: u32 = 0x0000_0004;

const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;

const O_RDONLY: i32 = 0;
const O_NOFOLLOW: i32 = 0x0100;
const O_DIRECTORY: i32 = 0x0010_0000;
const O_CLOEXEC: i32 = 0x0100_0000;

unsafe extern "C" {
    fn open(path: *const i8, flags: i32, ...) -> i32;
    fn close(fd: i32) -> i32;
    fn getattrlistbulk(
        dirfd: i32,
        attrs: *mut AttrList,
        buf: *mut u8,
        size: usize,
        options: u64,
    ) -> i32;
}

/// Closes the descriptor whatever happens.
struct Fd(i32);

impl Drop for Fd {
    fn drop(&mut self) {
        // SAFETY: the descriptor is owned by this guard and closed once.
        unsafe { close(self.0) };
    }
}

/// Reads a little-endian value at `at`, where attribute buffers only promise
/// 4-byte alignment.
fn read<T: Copy>(buf: &[u8], at: usize) -> Option<T> {
    if at + std::mem::size_of::<T>() > buf.len() {
        return None;
    }
    // SAFETY: bounds checked above; `read_unaligned` has no alignment need.
    Some(unsafe { std::ptr::read_unaligned(buf.as_ptr().add(at).cast::<T>()) })
}

/// Lists a directory with the attributes of every entry.
///
/// Entries that could not be read individually are skipped; an error opening
/// or reading the directory itself is returned, like `fs::read_dir`.
pub fn read_dir(dir: &Path) -> io::Result<Vec<Entry>> {
    let path = CString::new(dir.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: `path` is a valid NUL-terminated string for the whole call.
    let fd = unsafe {
        open(
            path.as_ptr(),
            O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = Fd(fd);

    let mut attrs = AttrList {
        bitmapcount: ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: ATTR_CMN_RETURNED_ATTRS
            | ATTR_CMN_NAME
            | ATTR_CMN_DEVID
            | ATTR_CMN_OBJTYPE
            | ATTR_CMN_FLAGS
            | ATTR_CMN_FILEID
            | ATTR_CMN_ERROR,
        volattr: 0,
        dirattr: 0,
        fileattr: ATTR_FILE_LINKCOUNT | ATTR_FILE_ALLOCSIZE,
        forkattr: 0,
    };

    let mut buf = vec![0u8; 64 * 1024];
    let mut entries = Vec::new();

    loop {
        // SAFETY: `attrs` and `buf` outlive the call, and `buf.len()` is the
        // size handed to the kernel.
        let count = unsafe { getattrlistbulk(fd.0, &mut attrs, buf.as_mut_ptr(), buf.len(), 0) };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        if count == 0 {
            return Ok(entries);
        }

        let mut at = 0usize;
        for _ in 0..count {
            let Some(length) = read::<u32>(&buf, at) else {
                return Ok(entries);
            };
            let record = &buf[at..(at + length as usize).min(buf.len())];
            at += length as usize;
            if let Some(entry) = parse(record) {
                entries.push(entry);
            }
        }
    }
}

/// One record: length, returned-attribute set, then the attributes in the
/// kernel's fixed order — error, name, device, type, flags, file id, then the
/// file attributes.
fn parse(record: &[u8]) -> Option<Entry> {
    let returned_common: u32 = read(record, 4)?;
    let returned_file: u32 = read(record, 4 + 12)?;
    let mut at = 4 + 20;

    if returned_common & ATTR_CMN_ERROR != 0 {
        let error: u32 = read(record, at)?;
        at += 4;
        if error != 0 {
            return None;
        }
    }

    let mut name = OsString::new();
    if returned_common & ATTR_CMN_NAME != 0 {
        let offset: i32 = read(record, at)?;
        let length: u32 = read(record, at + 4)?;
        let start = usize::try_from(at as i64 + i64::from(offset)).ok()?;
        // The length counts the trailing NUL.
        let end = start + (length as usize).saturating_sub(1);
        name = OsString::from_vec(record.get(start..end)?.to_vec());
        at += 8;
    }
    let mut dev = 0;
    if returned_common & ATTR_CMN_DEVID != 0 {
        dev = u64::from(read::<i32>(record, at)? as u32);
        at += 4;
    }
    let mut kind = Kind::Other;
    if returned_common & ATTR_CMN_OBJTYPE != 0 {
        kind = match read::<u32>(record, at)? {
            VREG => Kind::File,
            VDIR => Kind::Dir,
            VLNK => Kind::Symlink,
            _ => Kind::Other,
        };
        at += 4;
    }
    let mut flags = 0;
    if returned_common & ATTR_CMN_FLAGS != 0 {
        flags = read(record, at)?;
        at += 4;
    }
    let mut ino = 0;
    if returned_common & ATTR_CMN_FILEID != 0 {
        ino = read(record, at)?;
        at += 8;
    }

    let mut nlink = 1;
    if returned_file & ATTR_FILE_LINKCOUNT != 0 {
        nlink = read(record, at)?;
        at += 4;
    }
    let mut allocated = 0;
    if returned_file & ATTR_FILE_ALLOCSIZE != 0 {
        allocated = read::<i64>(record, at)?.max(0) as u64;
    }
    if kind == Kind::Dir {
        allocated = 0;
    }

    Some(Entry {
        name,
        kind,
        allocated,
        dev,
        ino,
        nlink,
        flags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn agrees_with_lstat() {
        let dir = std::env::temp_dir().join("detox-mac-test-bulk");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("small"), "x").unwrap();
        std::fs::write(dir.join("big"), vec![7u8; 300_000]).unwrap();
        std::fs::hard_link(dir.join("big"), dir.join("twin")).unwrap();
        std::os::unix::fs::symlink("small", dir.join("link")).unwrap();
        let long = "n".repeat(200);
        std::fs::write(dir.join(&long), "y").unwrap();

        let mut entries = read_dir(&dir).unwrap();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<_> = entries
            .iter()
            .map(|e| e.name.to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["big", "link", &long, "small", "sub", "twin"]);

        for entry in &entries {
            let meta = std::fs::symlink_metadata(dir.join(&entry.name)).unwrap();
            assert_eq!(entry.ino, meta.ino(), "{names:?}");
            assert_eq!(entry.dev, meta.dev());
            match entry.kind {
                Kind::Dir => assert!(meta.is_dir()),
                Kind::Symlink => assert!(meta.file_type().is_symlink()),
                Kind::File => {
                    assert!(meta.is_file());
                    assert_eq!(entry.allocated, meta.blocks() * 512);
                    assert_eq!(u64::from(entry.nlink), meta.nlink());
                }
                Kind::Other => panic!("unexpected kind"),
            }
        }
        assert!(read_dir(&dir.join("small")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
