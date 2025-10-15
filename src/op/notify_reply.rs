use crate::types::{NodeID, NotifyID};
use polyfuse_kernel::fuse_notify_retrieve_in;
use std::marker::PhantomData;

/// A reply to a `NOTIFY_RETRIEVE` notification.
#[derive(Debug)]
#[non_exhaustive]
pub struct NotifyReply<'op> {
    /// The unique ID of the corresponding notification message.
    pub unique: NotifyID,

    /// The inode number corresponding with the cache data.
    pub ino: NodeID,

    /// The starting position of the cache data.
    pub offset: u64,

    /// The length of the retrieved cache data.
    pub size: u32,

    _marker: PhantomData<&'op ()>,
}

impl<'op> super::Op<'op> for NotifyReply<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_notify_retrieve_in = cx.decoder.fetch()?;
        Ok(Self {
            unique: NotifyID::from_raw(cx.header.unique().into_raw()),
            ino: cx.header.nodeid().ok_or(super::Error::InvalidNodeID)?,
            offset: arg.offset,
            size: arg.size,
            _marker: PhantomData,
        })
    }
}
