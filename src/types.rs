//! Common type definitions.

use libc::dev_t;
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

/// The device ID.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct DeviceID {
    major: u32,
    minor: u32,
}

impl DeviceID {
    /// Create a `DeviceID` from the corresponding pair of major/minor versions.
    ///
    /// # Panics
    /// In the FUSE protocol, this identifier is encoded as a 32-bit integer.
    /// Therefore, this constructor will panic if the passed arguments is outside
    /// of the following range:
    /// * `major`: 12bits (0x000 - 0xFFF)
    /// * `minor`: 20bits (0x00000 - 0xFFFFF)
    #[inline]
    pub const fn new(major: u32, minor: u32) -> Self {
        const MAJOR_MAX: u32 = 0x1 << (32 - 20);
        const MINOR_MAX: u32 = 0x1 << 20;
        assert!(major < MAJOR_MAX, "DeviceID.major");
        assert!(minor < MINOR_MAX, "DeviceID.minor");
        Self { major, minor }
    }

    /// Return the major number of this ID.
    #[inline]
    pub const fn major(&self) -> u32 {
        self.major
    }

    /// Return the minor number of this ID.
    #[inline]
    pub const fn minor(&self) -> u32 {
        self.minor
    }

    /// Convert the value of userland `dev_t` to `DeviceID`.
    ///
    /// # Panics
    /// See also the documentation of `DeviceID::new` for
    /// the range limitations of major/minor numbers.
    #[inline]
    pub const fn from_userspace_dev(rdev: dev_t) -> Self {
        Self::new(libc::major(rdev), libc::minor(rdev))
    }

    #[inline]
    pub const fn from_kernel_dev(rdev: u32) -> Self {
        Self::new(rdev >> 20, rdev & 0xfff)
    }

    #[inline]
    pub const fn into_userspace_dev(self) -> dev_t {
        libc::makedev(self.major, self.minor)
    }

    #[inline]
    pub const fn into_kernel_dev(self) -> u32 {
        self.major << 20 | self.minor & 0xfff
    }
}
