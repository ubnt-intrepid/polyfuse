use crate::{
    buf::{FallbackBuf, ToParts as _, TryReceive as _},
    device::Device,
    init::{InitIn, KernelConfig, NegotiationError},
    mount::{Mount, MountOptions},
    msg::{send_msg, MessageKind},
    session::Session,
};
use polyfuse_kernel::{
    fuse_init_out, fuse_opcode, FUSE_KERNEL_MINOR_VERSION, FUSE_KERNEL_VERSION,
    FUSE_MIN_READ_BUFFER,
};
use rustix::io::Errno;
use std::{borrow::Cow, io, path::Path};
use zerocopy::{FromZeros as _, IntoBytes as _};

pub fn connect<P>(
    mountpoint: P,
    mountopts: MountOptions,
    mut config: KernelConfig,
) -> io::Result<(Session, Device, Mount)>
where
    P: Into<Cow<'static, Path>>,
{
    let (fd, mount) = crate::mount::mount(mountpoint.into(), mountopts)?;
    let device = Device::from_fd(fd);

    let mut buf = FallbackBuf::new(FUSE_MIN_READ_BUFFER as usize);
    loop {
        buf.try_receive(&mut &device)?;
        let (header, arg, _remains) = buf.to_parts();

        if !matches!(header.opcode(), Ok(fuse_opcode::FUSE_INIT)) {
            // 原理上、FUSE_INIT の処理が完了するまで他のリクエストが pop されることはない
            // - ref: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/fs/fuse/fuse_i.h?h=v6.15.9#n693
            // カーネル側の実装に問題があると解釈し、そのリクエストを単に無視する
            tracing::error!(
                    "ignore any filesystem operations received before FUSE_INIT handling (unique={}, opcode={:?})",
                    header.unique(),
                    header.opcode(),
                );
            continue;
        }

        let init_in = InitIn::from_bytes(arg).ok_or(Errno::INVAL)?;

        match crate::init::negotiate(&mut config, init_in) {
            Ok(()) => (),
            Err(NegotiationError::TooLargeProtocolVersion) => {
                let (major, minor) = init_in.protocol_version();
                // major version が大きい場合、カーネルにダウングレードを要求する
                tracing::debug!(
                    "The requested ABI version {}.{} is too large (expected version is {}.x)\n",
                    major,
                    minor,
                    FUSE_KERNEL_VERSION,
                );
                tracing::debug!("  -> Wait for a second INIT request with an older version.");
                let mut out = fuse_init_out::new_zeroed();
                out.major = FUSE_KERNEL_VERSION;
                out.minor = FUSE_KERNEL_MINOR_VERSION;
                send_msg(
                    &device,
                    MessageKind::Reply {
                        unique: header.unique(),
                        error: None,
                    },
                    out.as_bytes(),
                )?;
                continue;
            }
            Err(NegotiationError::TooSmallProtocolVersion) => {
                let (major, minor) = init_in.protocol_version();
                // バージョンが小さすぎる場合は、プロコトルエラーを報告する。
                tracing::error!(
                    "The requested ABI version {}.{} is too small (expected version is {}.x)",
                    major,
                    minor,
                    FUSE_KERNEL_VERSION,
                );
                send_msg(
                    &device,
                    MessageKind::Reply {
                        unique: header.unique(),
                        error: Some(Errno::PROTO),
                    },
                    (),
                )?;
                continue;
            }
        }

        let out = config.to_out();
        send_msg(
            &device,
            MessageKind::Reply {
                unique: header.unique(),
                error: None,
            },
            out.to_bytes(),
        )?;

        return Ok((Session::new(config), device, mount));
    }
}
