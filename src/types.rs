//! Common type definitions.

use std::fmt;

macro_rules! define_id_type {
    ($(
        $(#[$($meta:tt)*])*
        pub type $name:ident = $typ:ty;
    )*) => {$(
        $(#[$($meta)*])*
        #[derive(Copy, Clone, PartialEq, Eq, Hash)]
        #[repr(transparent)]
        pub struct $name {
            value: $typ,
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(self, f)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}#{}", stringify!($name), self.value)
            }
        }

        impl $name {
            pub const fn from_raw(value: $typ) -> Self {
                Self { value }
            }

            pub const fn into_raw(self) -> $typ {
                self.value
            }
        }
    )*};
}

define_id_type! {
    /// The request ID.
    pub type RequestID = u64;

    /// The notification ID.
    pub type NotifyID = u64;

    /// The inode number.
    pub type NodeID = u64;

    /// The file handle.
    pub type FileID = u64;

    /// The user ID.
    pub type UID = u32;

    /// The group ID.
    pub type GID = u32;

    /// The process ID.
    pub type PID = u32;

    /// The poll wakeup ID.
    pub type PollWakeupID = u64;
}

impl NodeID {
    pub const ROOT: Self = Self::from_raw(1);
}

impl UID {
    pub fn current() -> Self {
        Self::from_raw(unsafe { libc::getuid() })
    }

    pub fn effective() -> Self {
        Self::from_raw(unsafe { libc::geteuid() })
    }
}

impl GID {
    pub fn current() -> Self {
        Self::from_raw(unsafe { libc::getgid() })
    }

    pub fn effective() -> Self {
        Self::from_raw(unsafe { libc::getegid() })
    }
}

/// The lock owner ID.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct LockOwnerID {
    value: u64,
}

impl fmt::Debug for LockOwnerID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LockOwnerID").finish()
    }
}

impl LockOwnerID {
    pub(crate) const fn from_raw(value: u64) -> Self {
        Self { value }
    }
}
