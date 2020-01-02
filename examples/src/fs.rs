//! Linux-specific filesystem operations.

use polyfuse::DirEntry;
use std::{
    ffi::{CStr, CString, OsStr, OsString},
    io, mem,
    os::unix::prelude::*,
    path::PathBuf,
    ptr::NonNull,
};

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
            flags |= libc::AT_EMPTY_PATH;
        }
        let c_path = CString::new(path.as_bytes())?;
        let fd = syscall!(open(c_path.as_ptr(), flags))?;
        Ok(Self(fd))
    }

    pub fn openat(&self, path: impl AsRef<OsStr>, mut flags: libc::c_int) -> io::Result<Self> {
        let path = path.as_ref();
        if path.is_empty() {
            flags |= libc::AT_EMPTY_PATH;
        }
        let c_path = CString::new(path.as_bytes())?;
        let fd = syscall!(openat(self.0, c_path.as_ptr(), flags))?;
        Ok(Self(fd))
    }

    pub fn fstatat(
        &self,
        path: impl AsRef<OsStr>,
        mut flags: libc::c_int,
    ) -> io::Result<libc::stat> {
        let path = path.as_ref();
        if path.is_empty() {
            flags |= libc::AT_EMPTY_PATH;
        }
        let c_path = CString::new(path.as_bytes())?;
        let mut stat = mem::MaybeUninit::uninit();
        syscall!(fstatat(self.0, c_path.as_ptr(), stat.as_mut_ptr(), flags))?;
        Ok(unsafe { stat.assume_init() })
    }

    pub fn read_dir(&self) -> io::Result<ReadDir> {
        let fd = self.openat(".", libc::O_RDONLY)?;

        let dp = NonNull::new(unsafe { libc::fdopendir(fd.0) }) //
            .ok_or_else(io::Error::last_os_error)?;

        Ok(ReadDir {
            dir: Dir(dp),
            offset: 0,
            fd,
        })
    }

    pub fn readlink(&self, path: impl AsRef<OsStr>) -> io::Result<OsString> {
        let path = path.as_ref();
        let c_path = CString::new(path.as_bytes())?;

        let mut buf = vec![0u8; (libc::PATH_MAX + 1) as usize];
        let len = syscall!(readlinkat(
            self.0,
            c_path.as_ptr(),
            buf.as_mut_ptr().cast::<libc::c_char>(),
            buf.len()
        ))? as usize;
        if len >= buf.len() {
            return Err(io::Error::from_raw_os_error(libc::ENAMETOOLONG));
        }
        unsafe {
            buf.set_len(len);
        }
        Ok(OsString::from_vec(buf))
    }

    pub fn procname(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.as_raw_fd()))
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
            flags |= libc::AT_EMPTY_PATH;
        }

        let c_name = CString::new(name.as_bytes())?;
        let uid = uid.unwrap_or_else(|| 0u32.wrapping_sub(1));
        let gid = gid.unwrap_or_else(|| 0u32.wrapping_sub(1));

        syscall!(fchownat(self.as_raw_fd(), c_name.as_ptr(), uid, gid, flags,))?;

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
            flags |= libc::AT_EMPTY_PATH;
        }

        let c_name = CString::new(name.as_bytes())?;

        syscall!(utimensat(
            self.as_raw_fd(),
            c_name.as_ptr(),
            tv.as_ptr(),
            flags,
        ))?;

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
            flags |= libc::AT_EMPTY_PATH;
        }

        let c_name = CString::new(name.as_bytes())?;
        let c_newname = CString::new(newname.as_bytes())?;

        syscall!(linkat(
            self.as_raw_fd(),
            c_name.as_ptr(),
            newparent.as_raw_fd(),
            c_newname.as_ptr(),
            flags,
        ))?;

        Ok(())
    }

    pub fn mkdirat(&self, name: impl AsRef<OsStr>, mode: libc::mode_t) -> io::Result<()> {
        let c_name = CString::new(name.as_ref().as_bytes())?;
        syscall!(mkdirat(self.as_raw_fd(), c_name.as_ptr(), mode))?;
        Ok(())
    }

    pub fn mknodat(
        &self,
        name: impl AsRef<OsStr>,
        mode: libc::mode_t,
        rdev: libc::dev_t,
    ) -> io::Result<()> {
        let c_name = CString::new(name.as_ref().as_bytes())?;

        syscall!(mknodat(self.as_raw_fd(), c_name.as_ptr(), mode, rdev))?;

        Ok(())
    }

    pub fn symlinkat(&self, name: impl AsRef<OsStr>, link: impl AsRef<OsStr>) -> io::Result<()> {
        let c_name = CString::new(name.as_ref().as_bytes())?;
        let c_link = CString::new(link.as_ref().as_bytes())?;

        syscall!(symlinkat(
            c_link.as_ptr(),
            self.as_raw_fd(),
            c_name.as_ptr()
        ))?;

        Ok(())
    }

    pub fn unlinkat(&self, name: impl AsRef<OsStr>, flags: libc::c_int) -> io::Result<()> {
        let c_name = CString::new(name.as_ref().as_bytes())?;
        syscall!(unlinkat(self.as_raw_fd(), c_name.as_ptr(), flags))?;
        Ok(())
    }

    pub fn renameat(
        &self,
        name: impl AsRef<OsStr>,
        newparent: Option<&impl AsRawFd>,
        newname: impl AsRef<OsStr>,
    ) -> io::Result<()> {
        let c_name = CString::new(name.as_ref().as_bytes())?;
        let c_newname = CString::new(newname.as_ref().as_bytes())?;

        syscall!(renameat(
            self.as_raw_fd(),
            c_name.as_ptr(),
            newparent.map_or(self.as_raw_fd(), |p| p.as_raw_fd()),
            c_newname.as_ptr()
        ))?;

        Ok(())
    }
}

// ==== ReadDir ====

struct Dir(NonNull<libc::DIR>);

unsafe impl Send for ReadDir {}
unsafe impl Sync for ReadDir {}

pub struct ReadDir {
    dir: Dir,
    offset: u64,
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
        syscall!(fsync(self.fd.as_raw_fd())).map(drop)
    }

    pub fn sync_data(&self) -> io::Result<()> {
        syscall!(fdatasync(self.fd.as_raw_fd())).map(drop)
    }
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

                let mut entry = DirEntry::new(name, raw_entry.d_ino, raw_entry.d_off as u64);
                entry.set_typ(raw_entry.d_type as u32);

                return Some(Ok(entry));
            }
        }
    }
}

pub fn chmod(path: impl AsRef<OsStr>, mode: libc::mode_t) -> io::Result<()> {
    let c_path = CString::new(path.as_ref().as_bytes())?;
    syscall!(chmod(c_path.as_ptr(), mode)).map(drop)
}

pub fn fchmod(fd: &impl AsRawFd, mode: libc::mode_t) -> io::Result<()> {
    syscall!(fchmod(fd.as_raw_fd(), mode)).map(drop)
}

pub fn truncate(path: impl AsRef<OsStr>, length: libc::off_t) -> io::Result<()> {
    let c_path = CString::new(path.as_ref().as_bytes())?;
    syscall!(truncate(c_path.as_ptr(), length)).map(drop)
}

pub fn ftruncate(fd: &impl AsRawFd, length: libc::off_t) -> io::Result<()> {
    syscall!(ftruncate(fd.as_raw_fd(), length)).map(drop)
}

pub fn utimens(path: impl AsRef<OsStr>, tv: [libc::timespec; 2]) -> io::Result<()> {
    let c_path = CString::new(path.as_ref().as_bytes())?;
    syscall!(utimensat(libc::AT_FDCWD, c_path.as_ptr(), tv.as_ptr(), 0)).map(drop)
}

pub fn futimens(fd: &impl AsRawFd, tv: [libc::timespec; 2]) -> io::Result<()> {
    syscall!(futimens(fd.as_raw_fd(), tv.as_ptr())).map(drop)
}

pub fn link(
    name: impl AsRef<OsStr>,
    parent: &impl AsRawFd,
    newname: impl AsRef<OsStr>,
    flags: libc::c_int,
) -> io::Result<()> {
    let c_name = CString::new(name.as_ref().as_bytes())?;
    let c_newname = CString::new(newname.as_ref().as_bytes())?;
    syscall!(linkat(
        libc::AT_FDCWD,
        c_name.as_ptr(),
        parent.as_raw_fd(),
        c_newname.as_ptr(),
        flags,
    ))
    .map(drop)
}

pub fn flock(fd: &impl AsRawFd, op: libc::c_int) -> io::Result<()> {
    syscall!(flock(fd.as_raw_fd(), op)).map(drop)
}

pub fn posix_fallocate(
    fd: &impl AsRawFd,
    offset: libc::off_t,
    length: libc::off_t,
) -> io::Result<()> {
    let err = unsafe { libc::posix_fallocate(fd.as_raw_fd(), offset, length) };
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
    syscall!(removexattr(c_path.as_ptr(), c_name.as_ptr(),))?;
    Ok(())
}

pub fn fstatvfs(fd: &impl AsRawFd) -> io::Result<libc::statvfs> {
    let mut stbuf = mem::MaybeUninit::<libc::statvfs>::zeroed();
    syscall!(fstatvfs(fd.as_raw_fd(), stbuf.as_mut_ptr()))?;
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
