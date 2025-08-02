use crate::{
    mount::{
        unpriv::{self as mount, Fusermount},
        MountOptions,
    },
    nix,
};
use polyfuse_kernel::FUSE_DEV_IOC_CLONE;
use std::{ffi::CStr, io, os::unix::prelude::*, path::PathBuf};

const FUSE_DEV_NAME: &CStr = c"/dev/fuse";

/// A connection with the FUSE kernel driver.
#[derive(Debug)]
pub struct Device {
    fd: OwnedFd,
    fusermount: Option<Fusermount>,
    mountpoint: PathBuf,
    mountopts: MountOptions,
}

impl Device {
    /// Acquire the connection to the FUSE kernel driver associated with the specified mountpoint.
    pub fn mount(mountpoint: impl Into<PathBuf>, mountopts: MountOptions) -> io::Result<Self> {
        let mountpoint = mountpoint.into();
        let (fd, fusermount) = mount::mount(&mountpoint, &mountopts)?;
        Ok(Self {
            fd,
            fusermount,
            mountpoint,
            mountopts,
        })
    }

    pub fn unmount(&mut self) -> io::Result<()> {
        if let Some(fusermount) = self.fusermount.take() {
            // この場合、fusermount の終了にともない umount(2) が暗黙的に呼び出される。
            // なので、fd受信用の UnixStream を閉じてバックグラウンドの fusermount を終了する。
            let _st = fusermount.wait()?;
        } else {
            // fusermount は fd を受信した直後に終了しているので、明示的に umount(2) を呼ぶ必要がある。
            // 非特権プロセスなので `fusermount -u /path/to/mountpoint` を呼ぶことで間接的にアンマウントを行う
            mount::unmount(&self.mountpoint, &self.mountopts)?;
        }

        Ok(())
    }

    pub fn try_ioc_clone(&self) -> io::Result<ClonedConnection> {
        Ok(ClonedConnection {
            fd: fuse_ioc_clone(&self.fd)?,
        })
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let _ = self.unmount();
    }
}

impl AsFd for Device {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl AsRawFd for Device {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl io::Read for Device {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        nix::read(&self.fd, buf)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        nix::read_vectored(&self.fd, bufs)
    }
}

impl io::Write for Device {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        nix::write(&self.fd, buf)
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        nix::write_vectored(&self.fd, bufs)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl io::Read for &Device {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        nix::read(&self.fd, buf)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        nix::read_vectored(&self.fd, bufs)
    }
}

impl io::Write for &Device {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        nix::write(&self.fd, buf)
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        nix::write_vectored(&self.fd, bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct ClonedConnection {
    fd: OwnedFd,
}

impl ClonedConnection {
    pub fn try_ioc_clone(&self) -> io::Result<Self> {
        Ok(Self {
            fd: fuse_ioc_clone(&self.fd)?,
        })
    }
}

impl AsFd for ClonedConnection {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl AsRawFd for ClonedConnection {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl io::Read for ClonedConnection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        nix::read(&self.fd, buf)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        nix::read_vectored(&self.fd, bufs)
    }
}

impl io::Write for ClonedConnection {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        nix::write(&self.fd, buf)
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        nix::write_vectored(&self.fd, bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn fuse_ioc_clone(fd: &impl AsRawFd) -> io::Result<OwnedFd> {
    let newfd = syscall! { open(FUSE_DEV_NAME.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    syscall! { ioctl(newfd, FUSE_DEV_IOC_CLONE, &fd.as_raw_fd()) };
    Ok(unsafe { OwnedFd::from_raw_fd(newfd) })
}
