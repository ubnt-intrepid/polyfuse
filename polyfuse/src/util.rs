use std::time::{Duration, SystemTime};

#[inline]
pub(crate) fn make_system_time((sec, nsec): (u64, u32)) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::new(sec, nsec)
}

pub(crate) trait BuilderExt {
    fn if_some<T, F>(self, value: Option<T>, f: F) -> Self
    where
        Self: Sized,
        F: FnOnce(Self, T) -> Self,
    {
        match value {
            Some(value) => f(self, value),
            None => self,
        }
    }
}

impl<T> BuilderExt for T {}
