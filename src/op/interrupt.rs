use crate::types::RequestID;
use polyfuse_kernel::fuse_interrupt_in;
use std::marker::PhantomData;

const FUSE_INT_REQ_BIT: u64 = 1;

/// Interrupt a previous FUSE request.
#[derive(Debug)]
#[non_exhaustive]
pub struct Interrupt<'op> {
    /// Return the target unique ID to be interrupted.
    pub unique: RequestID,

    _marker: PhantomData<&'op ()>,
}

impl<'op> super::Op<'op> for Interrupt<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        let arg: &fuse_interrupt_in = cx.decoder.fetch()?;
        Ok(Interrupt {
            unique: RequestID::from_raw(arg.unique & !FUSE_INT_REQ_BIT),
            _marker: PhantomData,
        })
    }
}
