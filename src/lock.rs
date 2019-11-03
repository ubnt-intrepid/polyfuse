use futures::ready;
use std::{
    cell::UnsafeCell,
    sync::Arc,
    task::{self, Poll},
};
use tokio::sync::semaphore::{Permit, Semaphore};

#[derive(Debug)]
pub struct Lock<T> {
    inner: Arc<LockInner<T>>,
    permit: Permit,
}

#[derive(Debug)]
struct LockInner<T> {
    val: UnsafeCell<T>,
    semaphore: Semaphore,
}

impl<T> Clone for Lock<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            permit: Permit::new(),
        }
    }
}

impl<T> Drop for Lock<T> {
    fn drop(&mut self) {
        self.release_lock();
    }
}

impl<T> Lock<T> {
    pub fn new(val: T) -> Self {
        Self {
            inner: Arc::new(LockInner {
                val: UnsafeCell::new(val),
                semaphore: Semaphore::new(1),
            }),
            permit: Permit::new(),
        }
    }

    pub fn poll_lock_with<F, R>(&mut self, cx: &mut task::Context, f: F) -> Poll<R>
    where
        F: FnOnce(&mut task::Context, &mut T) -> Poll<R>,
    {
        ready!(self.poll_acquire_lock(cx));

        let val = unsafe { &mut (*self.inner.val.get()) };
        let ret = ready!(f(cx, val));

        self.release_lock();

        Poll::Ready(ret)
    }

    fn poll_acquire_lock(&mut self, cx: &mut task::Context) -> Poll<()> {
        ready!(self.permit.poll_acquire(cx, &self.inner.semaphore))
            .unwrap_or_else(|e| unreachable!("{}", e));
        Poll::Ready(())
    }

    fn release_lock(&mut self) {
        self.permit.release(&self.inner.semaphore);
    }
}
