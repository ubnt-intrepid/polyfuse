//! Linux-specific filesystem operations.

use libc::{AT_EMPTY_PATH, AT_FDCWD, ENAMETOOLONG, O_RDONLY, PATH_MAX};
use std::{
    ffi::{CStr, CString, OsStr, OsString},
    io, mem,
    os::unix::prelude::*,
    path::PathBuf,
    ptr::NonNull,
};

// copied from https://github.com/tokio-rs/mio/blob/master/mio/src/sys/unix/mod.rs
macro_rules! syscall {
    ($name:ident ( $($args:expr),* $(,)? )) => {
        match unsafe { libc::$name($($args),*) } {
            -1 => Err(io::Error::last_os_error()),
            ret => Ok(ret),
        }
    }
}

// ==== FileDesc ====

pub struct FileDesc(RawFd);

impl Drop for FileDesc {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0);
        }
    }
}

impl FromRawFd for FileDesc {
    #[inline]
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        Self(fd)
    }
}

impl AsRawFd for FileDesc {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

impl IntoRawFd for FileDesc {
    #[inline]
    fn into_raw_fd(self) -> RawFd {
        self.0
    }
}

impl FileDesc {
    pub fn open(path: impl AsRef<OsStr>, mut flags: libc::c_int) -> io::Result<Self> {
        let path = path.as_ref();
        if path.is_empty() {
            flags |= AT_EMPTY_PATH;
        }
        let c_path = CString::new(path.as_bytes())?;
        let fd = syscall!(open(c_path.as_ptr(), flags))?;
        Ok(Self(fd))
    }

    pub fn procname(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.as_raw_fd()))
    }

    pub fn openat(&self, path: impl AsRef<OsStr>, mut flags: libc::c_int) -> io::Result<Self> {
        let path = path.as_ref();
        if path.is_empty() {
            flags |= AT_EMPTY_PATH;
        }
        let fd = self.as_raw_fd();
        let c_path = CString::new(path.as_bytes())?;
        let fd = syscall!(openat(fd, c_path.as_ptr(), flags))?;
        Ok(Self(fd))
    }

    pub fn fstatat(
        &self,
        path: impl AsRef<OsStr>,
        mut flags: libc::c_int,
    ) -> io::Result<libc::stat> {
        let path = path.as_ref();
        if path.is_empty() {
            flags |= AT_EMPTY_PATH;
        }
        let fd = self.as_raw_fd();
        let c_path = CString::new(path.as_bytes())?;
        let mut stat = mem::MaybeUninit::uninit();
        syscall!(fstatat(fd, c_path.as_ptr(), stat.as_mut_ptr(), flags))?;
        Ok(unsafe { stat.assume_init() })
    }

    pub fn read_dir(&self) -> io::Result<ReadDir> {
        let fd = self.openat(".", O_RDONLY)?;

        // TODO: asyncify.
        let dp = NonNull::new(unsafe { libc::fdopendir(fd.0) }) //
            .ok_or_else(io::Error::last_os_error)?;

        Ok(ReadDir {
            dir: Dir(dp),
            offset: 0,
            fd,
        })
    }

    pub fn readlinkat(&self, path: impl AsRef<OsStr>) -> io::Result<OsString> {
        let fd = self.as_raw_fd();
        let path = path.as_ref();
        let c_path = CString::new(path.as_bytes())?;
        let mut buf = vec![0u8; (PATH_MAX + 1) as usize];
        let len = syscall!(readlinkat(
            fd,
            c_path.as_ptr(),
            buf.as_mut_ptr().cast::<libc::c_char>(),
            buf.len()
        ))? as usize;
        if len >= buf.len() {
            return Err(io::Error::from_raw_os_error(ENAMETOOLONG));
        }
        unsafe {
            buf.set_len(len);
        }
        Ok(OsString::from_vec(buf))
    }

    pub fn fchownat(
        &self,
        name: impl AsRef<OsStr>,
        uid: Option<libc::uid_t>,
        gid: Option<libc::gid_t>,
        mut flags: libc::c_int,
    ) -> io::Result<()> {
        let name = name.as_ref();
        if name.is_empty() {
            flags |= AT_EMPTY_PATH;
        }

        let fd = self.as_raw_fd();
        let c_name = CString::new(name.as_bytes())?;
        let uid = uid.unwrap_or_else(|| 0u32.wrapping_sub(1));
        let gid = gid.unwrap_or_else(|| 0u32.wrapping_sub(1));

        syscall!(fchownat(fd, c_name.as_ptr(), uid, gid, flags))?;

        Ok(())
    }

    pub fn futimensat(
        &self,
        name: impl AsRef<OsStr>,
        tv: [libc::timespec; 2],
        mut flags: libc::c_int,
    ) -> io::Result<()> {
        let name = name.as_ref();
        if name.is_empty() {
            flags |= AT_EMPTY_PATH;
        }

        let fd = self.as_raw_fd();
        let c_name = CString::new(name.as_bytes())?;

        syscall!(utimensat(fd, c_name.as_ptr(), tv.as_ptr(), flags))?;

        Ok(())
    }

    pub fn linkat(
        &self,
        name: impl AsRef<OsStr>,
        newparent: &impl AsRawFd,
        newname: impl AsRef<OsStr>,
        mut flags: libc::c_int,
    ) -> io::Result<()> {
        let name = name.as_ref();
        let newname = newname.as_ref();
        if name.is_empty() {
            flags |= AT_EMPTY_PATH;
        }

        let parent_fd = self.as_raw_fd();
        let newparent_fd = newparent.as_raw_fd();
        let c_name = CString::new(name.as_bytes())?;
        let c_newname = CString::new(newname.as_bytes())?;

        syscall!(linkat(
            parent_fd,
            c_name.as_ptr(),
            newparent_fd,
            c_newname.as_ptr(),
            flags,
        ))?;

        Ok(())
    }

    pub fn mkdirat(&self, name: impl AsRef<OsStr>, mode: libc::mode_t) -> io::Result<()> {
        let fd = self.as_raw_fd();
        let c_name = CString::new(name.as_ref().as_bytes())?;
        syscall!(mkdirat(fd, c_name.as_ptr(), mode))?;
        Ok(())
    }

    pub fn mknodat(
        &self,
        name: impl AsRef<OsStr>,
        mode: libc::mode_t,
        rdev: libc::dev_t,
    ) -> io::Result<()> {
        let fd = self.as_raw_fd();
        let c_name = CString::new(name.as_ref().as_bytes())?;
        syscall!(mknodat(fd, c_name.as_ptr(), mode, rdev))?;
        Ok(())
    }

    pub fn symlinkat(&self, name: impl AsRef<OsStr>, link: impl AsRef<OsStr>) -> io::Result<()> {
        let fd = self.as_raw_fd();
        let c_name = CString::new(name.as_ref().as_bytes())?;
        let c_link = CString::new(link.as_ref().as_bytes())?;
        syscall!(symlinkat(c_link.as_ptr(), fd, c_name.as_ptr()))?;
        Ok(())
    }

    pub fn unlinkat(&self, name: impl AsRef<OsStr>, flags: libc::c_int) -> io::Result<()> {
        let fd = self.as_raw_fd();
        let c_name = CString::new(name.as_ref().as_bytes())?;
        syscall!(unlinkat(fd, c_name.as_ptr(), flags))?;
        Ok(())
    }

    pub fn renameat(
        &self,
        name: impl AsRef<OsStr>,
        newparent: Option<&impl AsRawFd>,
        newname: impl AsRef<OsStr>,
    ) -> io::Result<()> {
        let parent_fd = self.as_raw_fd();
        let newparent_fd = newparent.map_or(parent_fd, |p| p.as_raw_fd());
        let c_name = CString::new(name.as_ref().as_bytes())?;
        let c_newname = CString::new(newname.as_ref().as_bytes())?;
        syscall!(renameat(
            parent_fd,
            c_name.as_ptr(),
            newparent_fd,
            c_newname.as_ptr()
        ))?;
        Ok(())
    }
}

// ==== ReadDir ====

struct Dir(NonNull<libc::DIR>);

impl Dir {
    fn fd(&self) -> RawFd {
        unsafe { libc::dirfd(self.0.as_ptr()) }
    }
}

unsafe impl Send for ReadDir {}
unsafe impl Sync for ReadDir {}

pub struct ReadDir {
    dir: Dir,
    offset: u64,
    #[allow(dead_code)]
    fd: FileDesc,
}

impl Drop for ReadDir {
    fn drop(&mut self) {
        unsafe {
            libc::closedir(self.dir.0.as_ptr());
        }
    }
}

impl ReadDir {
    pub fn seek(&mut self, offset: u64) {
        if offset != self.offset {
            unsafe {
                libc::seekdir(self.dir.0.as_mut(), offset as libc::off_t);
            }
            self.offset = offset;
        }
    }

    pub fn sync_all(&self) -> io::Result<()> {
        let fd = self.dir.fd();
        syscall!(fsync(fd))?;
        Ok(())
    }

    pub fn sync_data(&self) -> io::Result<()> {
        let fd = self.dir.fd();
        syscall!(fdatasync(fd))?;
        Ok(())
    }
}

pub struct DirEntry {
    pub name: OsString,
    pub ino: u64,
    pub typ: u32,
    pub off: u64,
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            loop {
                set_errno(0);
                let dp = libc::readdir(self.dir.0.as_mut());
                if dp.is_null() {
                    match errno() {
                        0 => return None, // end of stream
                        errno => return Some(Err(io::Error::from_raw_os_error(errno))),
                    }
                }

                let raw_entry = &*dp;

                let name = OsStr::from_bytes(CStr::from_ptr(raw_entry.d_name.as_ptr()).to_bytes());
                match name.as_bytes() {
                    b"." | b".." => continue,
                    _ => (),
                }

                let entry = DirEntry {
                    name: name.to_owned(),
                    ino: raw_entry.d_ino,
                    typ: raw_entry.d_type as u32,
                    off: raw_entry.d_off as u64,
                };

                return Some(Ok(entry));
            }
        }
    }
}

pub fn chmod(path: impl AsRef<OsStr>, mode: libc::mode_t) -> io::Result<()> {
    let c_path = CString::new(path.as_ref().as_bytes())?;
    syscall!(chmod(c_path.as_ptr(), mode))?;
    Ok(())
}

pub fn fchmod(fd: &impl AsRawFd, mode: libc::mode_t) -> io::Result<()> {
    let fd = fd.as_raw_fd();
    syscall!(fchmod(fd, mode))?;
    Ok(())
}

pub fn truncate(path: impl AsRef<OsStr>, length: libc::off_t) -> io::Result<()> {
    let c_path = CString::new(path.as_ref().as_bytes())?;
    syscall!(truncate(c_path.as_ptr(), length))?;
    Ok(())
}

pub fn ftruncate(fd: &impl AsRawFd, length: libc::off_t) -> io::Result<()> {
    let fd = fd.as_raw_fd();
    syscall!(ftruncate(fd, length))?;
    Ok(())
}

pub fn utimens(path: impl AsRef<OsStr>, tv: [libc::timespec; 2]) -> io::Result<()> {
    let c_path = CString::new(path.as_ref().as_bytes())?;
    syscall!(utimensat(AT_FDCWD, c_path.as_ptr(), tv.as_ptr(), 0))?;
    Ok(())
}

pub fn futimens(fd: &impl AsRawFd, tv: [libc::timespec; 2]) -> io::Result<()> {
    let fd = fd.as_raw_fd();
    syscall!(futimens(fd, tv.as_ptr()))?;
    Ok(())
}

pub fn link(
    name: impl AsRef<OsStr>,
    parent: &impl AsRawFd,
    newname: impl AsRef<OsStr>,
    flags: libc::c_int,
) -> io::Result<()> {
    let parent_fd = parent.as_raw_fd();
    let c_name = CString::new(name.as_ref().as_bytes())?;
    let c_newname = CString::new(newname.as_ref().as_bytes())?;
    syscall!(linkat(
        AT_FDCWD,
        c_name.as_ptr(),
        parent_fd,
        c_newname.as_ptr(),
        flags,
    ))?;
    Ok(())
}

pub fn flock(fd: &impl AsRawFd, op: libc::c_int) -> io::Result<()> {
    let fd = fd.as_raw_fd();
    syscall!(flock(fd, op))?;
    Ok(())
}

pub fn posix_fallocate(
    fd: &impl AsRawFd,
    offset: libc::off_t,
    length: libc::off_t,
) -> io::Result<()> {
    let fd = fd.as_raw_fd();
    let err = unsafe { libc::posix_fallocate(fd, offset, length) };
    if err != 0 {
        return Err(io::Error::from_raw_os_error(err));
    }
    Ok(())
}

pub fn getxattr(
    path: impl AsRef<OsStr>,
    name: impl AsRef<OsStr>,
    value: Option<&mut [u8]>,
) -> io::Result<usize> {
    let c_path = CString::new(path.as_ref().as_bytes())?;
    let c_name = CString::new(name.as_ref().as_bytes())?;
    let (value_ptr, size) = match value {
        Some(value) => (value.as_mut_ptr().cast(), value.len()),
        None => (std::ptr::null_mut(), 0),
    };
    syscall!(getxattr(
        c_path.as_ptr(), //
        c_name.as_ptr(),
        value_ptr,
        size
    ))
    .map(|size| size as usize)
}

pub fn listxattr(path: impl AsRef<OsStr>, value: Option<&mut [u8]>) -> io::Result<usize> {
    let c_path = CString::new(path.as_ref().as_bytes())?;
    let (value_ptr, size) = match value {
        Some(value) => (value.as_mut_ptr().cast(), value.len()),
        None => (std::ptr::null_mut(), 0),
    };
    syscall!(listxattr(c_path.as_ptr(), value_ptr, size)).map(|size| size as usize)
}

pub fn setxattr(
    path: impl AsRef<OsStr>,
    name: impl AsRef<OsStr>,
    value: &[u8],
    flags: libc::c_int,
) -> io::Result<()> {
    let c_path = CString::new(path.as_ref().as_bytes())?;
    let c_name = CString::new(name.as_ref().as_bytes())?;
    syscall!(setxattr(
        c_path.as_ptr(),
        c_name.as_ptr(),
        value.as_ptr().cast::<libc::c_void>(),
        value.len(),
        flags,
    ))?;
    Ok(())
}

pub fn removexattr(path: impl AsRef<OsStr>, name: impl AsRef<OsStr>) -> io::Result<()> {
    let c_path = CString::new(path.as_ref().as_bytes())?;
    let c_name = CString::new(name.as_ref().as_bytes())?;
    syscall!(removexattr(c_path.as_ptr(), c_name.as_ptr()))?;
    Ok(())
}

pub fn fstatvfs(fd: &impl AsRawFd) -> io::Result<libc::statvfs> {
    let fd = fd.as_raw_fd();
    let mut stbuf = mem::MaybeUninit::<libc::statvfs>::zeroed();
    syscall!(fstatvfs(fd, stbuf.as_mut_ptr()))?;
    Ok(unsafe { stbuf.assume_init() })
}

#[inline]
unsafe fn errno() -> i32 {
    *libc::__errno_location()
}

#[inline]
unsafe fn set_errno(errno: i32) {
    *libc::__errno_location() = errno;
}
