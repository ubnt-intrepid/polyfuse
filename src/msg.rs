use crate::{
    bytes::{Bytes as _, ToBytes, POD},
    types::RequestID,
};
use polyfuse_kernel::{fuse_notify_code, fuse_out_header};
use rustix::io::Errno;
use std::{io, mem};
use zerocopy::transmute;

pub enum MessageKind {
    Reply {
        unique: RequestID,
        error: Option<Errno>,
    },
    Notify {
        code: fuse_notify_code,
    },
}

pub fn send_msg<T, B>(conn: T, kind: MessageKind, arg: B) -> io::Result<()>
where
    T: io::Write,
    B: ToBytes,
{
    let arg = arg.to_bytes();

    let len = (mem::size_of::<fuse_out_header>() + arg.size())
        .try_into()
        .map_err(|_| Errno::INVAL)?;

    let (error, unique) = match kind {
        MessageKind::Reply { unique, error } => {
            let error = error.map_or(0, |e| -e.raw_os_error());
            (error, unique.into_raw())
        }
        MessageKind::Notify { code } => (transmute!(code), 0),
    };

    crate::bytes::write_bytes(conn, (POD(fuse_out_header { len, error, unique }), arg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[inline]
    fn bytes(bytes: &[u8]) -> &[u8] {
        bytes
    }
    macro_rules! b {
        ($($b:expr),*$(,)?) => ( *bytes(&[$($b),*]) );
    }

    #[test]
    fn send_reply_empty() {
        let mut buf = vec![0u8; 0];
        send_msg(
            &mut buf,
            MessageKind::Reply {
                unique: RequestID::from_raw(42),
                error: Some(Errno::INTR),
            },
            (),
        )
        .unwrap();
        assert_eq!(buf[0..4], b![0x10, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(buf[4..8], b![0xfc, 0xff, 0xff, 0xff], "header.error");
        assert_eq!(
            buf[8..16],
            b![0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
    }

    #[test]
    fn send_reply_single_data() {
        let mut buf = vec![0u8; 0];
        send_msg(
            &mut buf,
            MessageKind::Reply {
                unique: RequestID::from_raw(42),
                error: None,
            },
            "hello",
        )
        .unwrap();
        assert_eq!(buf[0..4], b![0x15, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(buf[4..8], b![0x00, 0x00, 0x00, 0x00], "header.error");
        assert_eq!(
            buf[8..16],
            b![0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
        assert_eq!(buf[16..], b![0x68, 0x65, 0x6c, 0x6c, 0x6f], "payload");
    }

    #[test]
    fn send_msg_chunked_data() {
        let payload: &[&[u8]] = &[
            "hello, ".as_ref(), //
            "this ".as_ref(),
            "is a ".as_ref(),
            "message.".as_ref(),
        ];
        let mut buf = vec![0u8; 0];
        send_msg(
            &mut buf,
            MessageKind::Reply {
                unique: RequestID::from_raw(26),
                error: None,
            },
            payload,
        )
        .unwrap();
        assert_eq!(buf[0..4], b![0x29, 0x00, 0x00, 0x00], "header.len");
        assert_eq!(buf[4..8], b![0x00, 0x00, 0x00, 0x00], "header.error");
        assert_eq!(
            buf[8..16],
            b![0x1a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            "header.unique"
        );
        assert_eq!(buf[16..], *b"hello, this is a message.", "payload");
    }
}
