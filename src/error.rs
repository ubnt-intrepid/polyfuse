pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub struct Error(pub i32);

impl Error {
    pub const NOSYS: Self = Self(libc::ENOSYS);
}
