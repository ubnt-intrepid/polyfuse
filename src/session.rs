use crate::{
    buf::{FallbackBuf, InHeader, SpliceBuf, ToParts, TryReceive},
    bytes::Bytes,
    init::{KernelConfig, KernelFlags},
    msg::{send_msg, MessageKind},
    op::{DecodeError, Operation},
    reply::ReplySender,
    types::NotifyID,
};
use polyfuse_kernel::*;
use rustix::io::Errno;
use std::{
    io, mem,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

// ==== Session ====

/// The object containing the contextrual information about a FUSE session.
#[derive(Debug)]
pub struct Session {
    config: KernelConfig,
    exited: AtomicBool,
    notify_unique: AtomicU64,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.exit();
    }
}

impl Session {
    pub(crate) const fn new(config: KernelConfig) -> Self {
        Self {
            config,
            exited: AtomicBool::new(false),
            notify_unique: AtomicU64::new(0),
        }
    }

    #[inline]
    pub fn config(&self) -> &KernelConfig {
        &self.config
    }

    #[inline]
    fn request_buffer_size(&self) -> usize {
        mem::size_of::<fuse_in_header>()
            + mem::size_of::<fuse_write_in>()
            + self.config.max_write as usize
    }

    pub fn new_splice_buffer(&self) -> io::Result<SpliceBuf> {
        if self.config.flags.contains(KernelFlags::SPLICE_READ) {
            SpliceBuf::new(self.request_buffer_size())
        } else {
            Err(Errno::NOTSUP.into())
        }
    }

    pub fn new_fallback_buffer(&self) -> FallbackBuf {
        FallbackBuf::new(self.request_buffer_size())
    }

    #[inline]
    pub fn exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    #[inline]
    pub fn exit(&self) {
        self.exited.store(true, Ordering::Release)
    }

    /// Receive an incoming FUSE request from the kernel.
    pub fn recv_request<T, B>(&self, mut conn: T, buf: &mut B) -> io::Result<bool>
    where
        B: TryReceive<T>,
    {
        if self.exited() {
            return Ok(false);
        }

        let header = match buf.try_receive(&mut conn) {
            Err(err) => match Errno::from_io_error(&err) {
                Some(Errno::NODEV) => {
                    self.exit();
                    return Ok(false);
                }
                _ => return Err(err),
            },
            Ok(header) => header,
        };

        match header.opcode() {
            Ok(fuse_opcode::FUSE_INIT) => {
                // FUSE_INIT リクエストは Session の初期化時に処理しているはずなので、ここで読み込まれることはないはず
                tracing::error!("unexpected FUSE_INIT request received");
                return Err(Errno::PROTO.into());
            }

            Ok(fuse_opcode::FUSE_DESTROY) => {
                // TODO: FUSE_DESTROY 後にリクエストの読み込みを中断するかどうかを決める
                tracing::debug!("FUSE_DESTROY received");
                self.exit();
                return Ok(false);
            }
            _ => (),
        }

        Ok(true)
    }

    /// Decode the arguments of FUSE request stored buffer, and then start the request handling.
    ///
    /// Note that the instance of `conn` must be the same as the one used
    /// when the corresponding request was received via [`Session::recv_request`].
    /// If anything else (including cloning with `FUSE_IOC_CLONE`) is specified,
    /// the corresponding kernel processing will be isolated, and the process
    /// that issued the associated syscall may enter a deadlock state.
    pub fn decode<'req, T, B>(
        &'req self,
        conn: T,
        buf: &'req mut B,
    ) -> Result<RequestParts<'req, T, B>, DecodeError>
    where
        T: io::Write,
        B: ToParts,
    {
        let (header, arg, remains) = buf.to_parts();
        let op = match Operation::decode(&self.config, header, arg, remains) {
            Ok(op) => Some(op),
            Err(DecodeError::UnsupportedOpcode) => None,
            Err(err) => return Err(err),
        };
        Ok((
            Request {
                session: self,
                header,
                conn,
            },
            op,
        ))
    }

    fn handle_reply_error(&self, err: io::Error) -> io::Result<()> {
        match Errno::from_io_error(&err) {
            Some(Errno::NODEV) => {
                // 切断済みであれば無視
                self.exit();
                Ok(())
            }
            Some(Errno::NOENT) => Ok(()),
            _ => Err(err),
        }
    }

    pub fn notifier<T>(&self, conn: T) -> Notifier<'_, T>
    where
        T: io::Write,
    {
        Notifier {
            session: self,
            conn,
        }
    }
}

pub type RequestParts<'req, T, B> = (
    Request<'req, T>,
    Option<Operation<'req, <B as ToParts>::Data<'req>>>,
);

pub struct Request<'req, T> {
    session: &'req Session,
    header: &'req InHeader,
    conn: T,
}

impl<T> Request<'_, T> {
    pub fn header(&self) -> &InHeader {
        self.header
    }
}

impl<T> ReplySender for Request<'_, T>
where
    T: io::Write,
{
    fn config(&self) -> &KernelConfig {
        self.session.config()
    }

    fn reply_raw<B>(self, error: Option<Errno>, arg: B) -> io::Result<()>
    where
        B: Bytes,
    {
        send_msg(
            self.conn,
            MessageKind::Reply {
                unique: self.header.unique(),
                error,
            },
            arg,
        )
        .or_else(|err| self.session.handle_reply_error(err))
    }
}

pub struct Notifier<'sess, T> {
    session: &'sess Session,
    conn: T,
}

impl<T> crate::notify::Notifier for Notifier<'_, T>
where
    T: io::Write,
{
    fn send<B>(self, code: fuse_notify_code, arg: B) -> io::Result<()>
    where
        B: Bytes,
    {
        send_msg(self.conn, MessageKind::Notify { code }, arg)
            .or_else(|err| self.session.handle_reply_error(err))
    }

    fn new_notify_unique(&self) -> NotifyID {
        NotifyID::from_raw(self.session.notify_unique.fetch_add(1, Ordering::AcqRel))
    }
}
