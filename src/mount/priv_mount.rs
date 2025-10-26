use super::{FuseFlags, MountOptions};
use crate::{conn::FUSE_DEV_NAME, util::IteratorJoinExt as _};
use rustix::{
    fs::{Gid, Mode, OFlags, Stat, Uid},
    io::Errno,
    mount::UnmountFlags,
    process::{getgid, getuid},
    thread::CapabilitySet,
};
use std::{borrow::Cow, ffi::CString, io, os::unix::prelude::*, path::Path};

#[derive(Debug)]
pub(crate) struct PrivMount {
    _priv: (),
}

impl PrivMount {
    pub(crate) fn mount(
        mountpoint: &Path,
        stat: &Stat,
        mountopts: &MountOptions,
    ) -> io::Result<(OwnedFd, Self)> {
        let caps = rustix::thread::capabilities(None)?;
        if !caps.effective.contains(CapabilitySet::SYS_ADMIN) {
            return Err(Errno::PERM.into());
        }

        let fd = rustix::fs::open(FUSE_DEV_NAME, OFlags::RDWR | OFlags::CLOEXEC, Mode::empty())?;

        let PrivilegedOptions { fstype, opts } = encode_priv_options(
            mountopts,
            Some(&prefix(&fd, stat.st_mode, getuid(), getgid())),
        );

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
            mountpoint,
            fstype,
            rustix::mount::MountFlags::empty(),
            data.as_deref(),
        )?;

        Ok((fd, Self { _priv: () }))
    }

    pub(crate) fn unmount(self, mountpoint: &Path, _: &MountOptions) -> io::Result<()> {
        rustix::mount::unmount(mountpoint, UnmountFlags::DETACH)?;
        Ok(())
    }
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

struct PrivilegedOptions {
    pub fstype: String,
    pub opts: String,
}

fn encode_priv_options(opts: &MountOptions, prefix: Option<&str>) -> PrivilegedOptions {
    let mut fstype: String = if opts.fuse_flags.contains(FuseFlags::BLKDEV) {
        "fuseblk".into()
    } else {
        "fuse".into()
    };
    if let Some(subtype) = &opts.subtype {
        fstype.push('.');
        fstype.push_str(subtype.trim());
    }

    let opts = prefix
        .map(Cow::Borrowed)
        .into_iter()
        .chain(opts.iter_flags(false))
        .chain(opts.iter_common_opts())
        .join(",");

    PrivilegedOptions { fstype, opts }
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

    #[test]
    fn mount_opts_encode_privileged_default() {
        let opts = MountOptions::new();
        let dst = encode_priv_options(&opts, None);
        assert_eq!(dst.fstype, "fuse");
        assert_eq!(dst.opts, "");
    }

    #[test]
    fn encode_privileged_blkdev() {
        let mut opts = MountOptions::new();
        opts.fuse_flags |= FuseFlags::BLKDEV;

        let dst = encode_priv_options(&opts, None);
        assert_eq!(dst.fstype, "fuseblk");
        assert_eq!(dst.opts, "");
    }

    #[test]
    fn encode_privileged_subtype() {
        let mut opts = MountOptions::new();
        opts.fuse_flags |= FuseFlags::DEFAULT_PERMISSIONS | FuseFlags::ALLOW_OTHER;
        opts.subtype = Some("myfs".into());
        opts.max_read = Some(12);
        opts.blksize = Some(1024);

        let dst = encode_priv_options(&opts, Some("fd=9987,rootmode=444,user_id=0,group_id=0"));
        assert_eq!(dst.fstype, "fuse.myfs");
        assert_eq!(
            dst.opts,
            "fd=9987,rootmode=444,user_id=0,group_id=0,default_permissions,allow_other,blksize=1024,max_read=12"
        );
    }
}
