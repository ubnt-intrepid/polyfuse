use libc::{c_int, c_void};
use std::{
    borrow::Cow,
    ffi::OsStr,
    fmt, io,
    mem::{self, MaybeUninit},
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::{net::UnixStream, prelude::*},
    },
    path::{Path, PathBuf},
    process::{Child, Command},
    ptr,
};

// refs:
// * https://github.com/libfuse/libfuse/blob/fuse-3.10.5/lib/mount.c
// * https://github.com/libfuse/libfuse/blob/fuse-3.10.5/util/fusermount.c
// * https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/fs/fuse/inode.c?h=v6.15.9

const FUSERMOUNT_PROG: &str = "/usr/bin/fusermount";
const FUSE_COMMFD_ENV: &str = "_FUSE_COMMFD";

#[derive(Debug, Clone)]
pub struct MountOptions {
    options: Vec<String>,

    // FUSE-specific options
    default_permissions: bool,
    allow_other: bool,
    blksize: Option<u32>,
    max_read: Option<u32>,

    // fusermount-specific options
    auto_unmount: bool,
    blkdev: bool,
    fsname: Option<String>,
    fusermount_path: Option<PathBuf>,
}

impl Default for MountOptions {
    fn default() -> Self {
        Self {
            options: vec![],
            default_permissions: false,
            allow_other: false,
            blksize: None,
            max_read: None,
            auto_unmount: true,
            blkdev: false,
            fsname: None,
            fusermount_path: None,
        }
    }
}

impl MountOptions {
    pub fn default_permissions(&mut self, enabled: bool) -> &mut Self {
        self.default_permissions = enabled;
        self
    }

    pub fn allow_other(&mut self, enabled: bool) -> &mut Self {
        self.allow_other = enabled;
        self
    }

    pub fn blksize(&mut self, blksize: u32) -> &mut Self {
        self.blksize = Some(blksize);
        self
    }

    pub fn max_read(&mut self, max_read: u32) -> &mut Self {
        self.max_read = Some(max_read);
        self
    }

    pub fn auto_unmount(&mut self, enabled: bool) -> &mut Self {
        self.auto_unmount = enabled;
        self
    }

    pub fn blkdev(&mut self, enabled: bool) -> &mut Self {
        self.blkdev = enabled;
        self
    }

    pub fn fsname(&mut self, fsname: &str) -> &mut Self {
        // FIXME: validatation
        self.fsname = Some(fsname.to_owned());
        self
    }

    pub fn fusermount_path(&mut self, program: impl AsRef<OsStr>) -> &mut Self {
        let program = Path::new(program.as_ref());
        assert!(
            program.is_absolute(),
            "the binary path to `fusermount` must be absolute."
        );
        self.fusermount_path = Some(program.to_owned());
        self
    }

    pub fn mount_option(&mut self, option: &str) -> &mut Self {
        for option in option.split(',').map(|s| s.trim()) {
            match option {
                "auto_unmount" => {
                    self.auto_unmount(true);
                }
                "default_permissions" => {
                    self.default_permissions(true);
                }
                "allow_other" => {
                    self.allow_other(true);
                }
                option => self.options.push(option.to_owned()),
            }
        }
        self
    }
}

impl fmt::Display for MountOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use std::fmt::Write as _;

        let opts = self
            .options
            .iter()
            .map(|opt| Cow::Borrowed(opt.as_str()))
            .chain(
                self.default_permissions
                    .then_some(Cow::Borrowed("default_permissions")),
            )
            .chain(self.allow_other.then_some(Cow::Borrowed("allow_other")))
            .chain(self.blksize.map(|n| format!("blksize={}", n).into()))
            .chain(self.max_read.map(|n| format!("max_read={}", n).into()))
            .chain(self.auto_unmount.then_some(Cow::Borrowed("auto_unmount")))
            .chain(self.blkdev.then_some(Cow::Borrowed("blkdev")))
            .chain(
                self.fsname
                    .as_deref()
                    .map(|fsname| Cow::Owned(format!("fsname={}", fsname))),
            );

        for (i, opts) in opts.enumerate() {
            if i > 0 {
                f.write_char(',')?;
            }
            f.write_str(&*opts)?;
        }
        Ok(())
    }
}

fn fusermount_path(opts: &MountOptions) -> &Path {
    opts.fusermount_path
        .as_deref()
        .unwrap_or_else(|| Path::new(FUSERMOUNT_PROG))
}

#[derive(Debug)]
pub struct Fusermount {
    child: Option<PipedChild>,
    mountpoint: PathBuf,
    mountopts: MountOptions,
}

impl Fusermount {
    pub fn unmount(mut self) -> io::Result<()> {
        self.unmount_()
    }

    fn unmount_(&mut self) -> io::Result<()> {
        if let Some(child) = self.child.take() {
            // この場合、fusermount の終了にともない umount(2) が暗黙的に呼び出される。
            // なので、fd受信用の UnixStream を閉じてバックグラウンドの fusermount を終了する。
            child.wait()?;
        } else {
            // fusermount は fd を受信した直後に終了しているので、明示的に umount(2) を呼ぶ必要がある。
            // 非特権プロセスなので `fusermount -u /path/to/mountpoint` を呼ぶことで間接的にアンマウントを行う
            unmount(&self.mountpoint, &self.mountopts)?;
        }
        Ok(())
    }
}

impl Drop for Fusermount {
    fn drop(&mut self) {
        let _ = self.unmount_();
    }
}

#[derive(Debug)]
struct PipedChild {
    child: Child,
    input: UnixStream,
}

impl PipedChild {
    fn wait(mut self) -> io::Result<()> {
        drop(self.input);
        let _st = self.child.wait()?;
        Ok(())
    }
}

/// Acquire the connection to the FUSE kernel driver associated with the specified mountpoint.
pub fn mount(mountpoint: PathBuf, mountopts: MountOptions) -> io::Result<(OwnedFd, Fusermount)> {
    let mut fusermount = Command::new(fusermount_path(&mountopts));

    let opts = mountopts.to_string();
    if !opts.is_empty() {
        fusermount.arg("-o").arg(opts);
    }

    fusermount.arg("--").arg(&mountpoint);

    let (input, output) = UnixStream::pair()?;
    let output = output.into_raw_fd();

    fusermount.env(FUSE_COMMFD_ENV, output.to_string());

    unsafe {
        fusermount.pre_exec(move || {
            syscall! { fcntl(output, libc::F_SETFD, 0) };
            Ok(())
        });
    }

    let child = fusermount.spawn()?;

    let fd = receive_fd(&input)?;

    let mut child = Some(PipedChild { child, input });
    if !mountopts.auto_unmount {
        // When auto_unmount is not specified, `fusermount` exits immediately
        // after sending the file descriptor and thus we need to wait until
        // the command is exited.
        let child = child.take().unwrap();
        child.wait()?;
    }

    Ok((
        fd,
        Fusermount {
            child,
            mountpoint,
            mountopts,
        },
    ))
}

fn receive_fd(reader: &UnixStream) -> io::Result<OwnedFd> {
    let mut buf = [0u8; 1];
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr() as *mut c_void,
        iov_len: 1,
    };

    #[repr(C)]
    struct Cmsg {
        header: libc::cmsghdr,
        fd: c_int,
    }
    let mut cmsg = MaybeUninit::<Cmsg>::uninit();

    let mut msg = libc::msghdr {
        msg_name: ptr::null_mut(),
        msg_namelen: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        msg_control: cmsg.as_mut_ptr() as *mut c_void,
        msg_controllen: mem::size_of_val(&cmsg),
        msg_flags: 0,
    };

    syscall! { recvmsg(reader.as_raw_fd(), &mut msg, 0) };

    if msg.msg_controllen < mem::size_of_val(&cmsg) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "too short control message length",
        ));
    }
    let cmsg = unsafe { cmsg.assume_init() };

    if cmsg.header.cmsg_type != libc::SCM_RIGHTS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "got control message with unknown type",
        ));
    }

    let fd = cmsg.fd;
    syscall! { fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };

    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn unmount(mountpoint: &Path, mountopts: &MountOptions) -> io::Result<()> {
    let _st = Command::new(fusermount_path(mountopts))
        .args(&["-u", "-q", "-z", "--"])
        .arg(mountpoint)
        .status()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_opts_encode() {
        let opts = MountOptions::default();
        assert_eq!(opts.to_string(), "auto_unmount");

        let mut opts = MountOptions::default();
        opts.auto_unmount(false);
        assert_eq!(opts.to_string(), "");

        let mut opts = MountOptions::default();
        opts.blkdev(true);
        opts.fsname("bradbury");
        assert_eq!(opts.to_string(), "auto_unmount,blkdev,fsname=bradbury");

        let mut opts = MountOptions::default();
        opts.mount_option("noasync");
        opts.default_permissions(true);
        assert_eq!(opts.to_string(), "noasync,default_permissions,auto_unmount");

        let mut opts = MountOptions::default();
        opts.default_permissions(true);
        opts.blksize(32);
        opts.max_read(11);
        assert_eq!(
            opts.to_string(),
            "default_permissions,blksize=32,max_read=11,auto_unmount"
        );

        let mut opts = MountOptions::default();
        opts.mount_option("noasync");
        opts.default_permissions(true);
        assert_eq!(opts.to_string(), "noasync,default_permissions,auto_unmount");
    }

    #[test]
    fn mount_options_parse() {
        let mut opts = MountOptions::default();
        opts.auto_unmount(false);
        assert!(!opts.auto_unmount);
        assert!(!opts.default_permissions);
        assert!(!opts.allow_other);
        assert!(opts.options.is_empty());

        opts.mount_option("noatime, auto_unmount, default_permissions, allow_other");
        assert!(opts.auto_unmount);
        assert!(opts.default_permissions);
        assert!(opts.allow_other);
        assert_eq!(opts.options.len(), 1);
        assert_eq!(opts.options[0], "noatime");
    }
}
