use crate::{
    conn::FUSE_DEV_NAME,
    mount::{MountOptions, PrivilegedOptions},
    Connection,
};
use rustix::{
    fs::{Gid, Mode, OFlags, Uid},
    io::Errno,
    mount::{MountFlags, UnmountFlags},
    process::{getgid, getuid},
    thread::CapabilitySet,
};
use std::{borrow::Cow, ffi::CString, io, os::unix::prelude::*, path::Path};

#[derive(Debug)]
#[must_use]
pub struct SysMount {
    mountpoint: Cow<'static, Path>,
    unmounted: bool,
}

impl SysMount {
    fn unmount_(&mut self) -> io::Result<()> {
        if !self.unmounted {
            rustix::mount::unmount(
                &*self.mountpoint,
                UnmountFlags::FORCE | UnmountFlags::DETACH,
            )?;
            self.unmounted = true;
        }
        Ok(())
    }

    pub fn unmount(mut self) -> io::Result<()> {
        self.unmount_()
    }
}

impl Drop for SysMount {
    fn drop(&mut self) {
        let _ = self.unmount_();
    }
}

/// Establish a connection with the FUSE kernel driver in privileged mode.
#[allow(clippy::ptr_arg)]
pub fn mount(
    mountpoint: &Cow<'static, Path>,
    mountopts: &MountOptions,
) -> io::Result<(Connection, SysMount)> {
    let caps = rustix::thread::capabilities(None)?;
    if !caps.effective.contains(CapabilitySet::SYS_ADMIN) {
        return Err(Errno::PERM.into());
    }

    let stat = rustix::fs::stat(&**mountpoint)?;

    let fd = rustix::fs::open(FUSE_DEV_NAME, OFlags::RDWR | OFlags::CLOEXEC, Mode::empty())?;

    let PrivilegedOptions { fstype, opts } =
        mountopts.privileged_options(Some(&prefix(&fd, stat.st_mode, getuid(), getgid())));

    let source = mountopts
        .fsname
        .as_deref()
        .or(mountopts.subtype.as_deref())
        .unwrap_or(
            FUSE_DEV_NAME
                .to_str()
                .expect("DEV_NAME must be a valid UTF-8 string"),
        );

    let data = Some(CString::new(opts).expect("invalid opts"));

    rustix::mount::mount(
        source,
        &**mountpoint,
        fstype,
        MountFlags::empty(),
        data.as_deref(),
    )?;

    Ok((
        Connection::from(fd),
        SysMount {
            mountpoint: mountpoint.clone(),
            unmounted: false,
        },
    ))
}

fn prefix(fd: &impl AsFd, mode: u32, uid: Uid, gid: Gid) -> String {
    let root_mode = mode & libc::S_IFMT;
    format!(
        "fd={},rootmode={:o},user_id={},group_id={}",
        fd.as_fd().as_raw_fd(),
        root_mode,
        uid,
        gid
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::ManuallyDrop;

    #[test]
    fn test_prefix() {
        let fd = unsafe { ManuallyDrop::new(OwnedFd::from_raw_fd(42)) };
        let mode = libc::S_IFDIR;
        let uid = Uid::from_raw(1001);
        let gid = Gid::from_raw(422);
        assert_eq!(
            prefix(&*fd, mode, uid, gid),
            "fd=42,rootmode=40000,user_id=1001,group_id=422"
        );
    }
}
