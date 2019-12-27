use std::{
    mem, slice,
    time::{Duration, SystemTime},
};

#[inline(always)]
pub(crate) unsafe fn as_bytes<T: Sized>(t: &T) -> &[u8] {
    slice::from_raw_parts(t as *const T as *const u8, mem::size_of::<T>())
}

#[inline(always)]
pub(crate) unsafe fn as_bytes_mut<T: Sized>(t: &mut T) -> &mut [u8] {
    slice::from_raw_parts_mut(t as *mut T as *mut u8, mem::size_of::<T>())
}

#[inline]
pub(crate) fn make_system_time((sec, nsec): (u64, u32)) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::new(sec, nsec)
}

#[inline]
pub(crate) fn make_raw_time(time: SystemTime) -> (u64, u32) {
    let duration = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("invalid time");
    (duration.as_secs(), duration.subsec_nanos())
}
