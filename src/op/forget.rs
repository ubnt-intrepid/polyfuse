use polyfuse_kernel::{fuse_batch_forget_in, fuse_forget_in, fuse_forget_one, fuse_opcode};
use std::fmt;

use crate::types::NodeID;

/// A set of forget information removed from the kernel's internal caches.
pub enum Forgets<'op> {
    Single([Forget; 1]),
    Batch(&'op [Forget]),
}

impl<'op> super::Op<'op> for Forgets<'op> {
    fn decode(cx: &mut super::Context<'op>) -> Result<Self, super::Error> {
        match cx.opcode {
            fuse_opcode::FUSE_FORGET => {
                let arg: &fuse_forget_in = cx.decoder.fetch()?;
                let forget = Forget {
                    raw: fuse_forget_one {
                        nodeid: cx
                            .header
                            .nodeid()
                            .ok_or(super::Error::InvalidNodeID)?
                            .into_raw(),
                        nlookup: arg.nlookup,
                    },
                };
                Ok(Forgets::Single([forget; 1]))
            }

            fuse_opcode::FUSE_BATCH_FORGET => {
                let arg: &fuse_batch_forget_in = cx.decoder.fetch()?;
                let forgets = cx
                    .decoder
                    .fetch_array::<fuse_forget_one>(arg.count as usize)?;
                let forgets = unsafe {
                    // Safety: `Forget` has the same layout with `fuse_forget_one`.
                    std::slice::from_raw_parts(forgets.as_ptr().cast(), forgets.len())
                };
                Ok(Forgets::Batch(forgets))
            }
            _ => unreachable!(),
        }
    }
}

impl fmt::Debug for Forgets<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.as_ref()).finish()
    }
}

impl<'op> std::ops::Deref for Forgets<'op> {
    type Target = [Forget];

    #[inline]
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Single(forget) => forget,
            Self::Batch(forgets) => forgets,
        }
    }
}

/// A forget information.
#[repr(transparent)]
pub struct Forget {
    raw: fuse_forget_one,
}

impl fmt::Debug for Forget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Forget")
            .field("ino", &self.ino())
            .field("nlookup", &self.nlookup())
            .finish()
    }
}

impl Forget {
    /// Return the inode number of the target inode.
    #[inline]
    pub fn ino(&self) -> NodeID {
        NodeID::from_raw(self.raw.nodeid).expect("invalid nodeid")
    }

    /// Return the released lookup count of the target inode.
    #[inline]
    pub fn nlookup(&self) -> u64 {
        self.raw.nlookup
    }
}
