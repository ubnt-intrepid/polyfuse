use futures::lock::Mutex;
use std::{
    collections::VecDeque,
    ops::{Deref, DerefMut},
    sync::{Arc, Weak},
};
use tokio::sync::mpsc;

// FIXME: handling for invalidated entries.

type Key = usize;

/// An object pool.
#[derive(Debug, Clone)]
pub struct Pool<T>(Arc<PoolInner<T>>);

#[derive(Debug)]
struct PoolInner<T> {
    /// The length of internal buffers.
    bufsize: usize,

    /// A channel sender for returning used entries.
    ///
    /// This value must be outside of `PoolState<T>` since
    /// `state` is locked while acquiring the unused entries.
    tx_used_entries: mpsc::Sender<EntryInner<T>>,

    /// The mutable state.
    state: Mutex<PoolState<T>>,
}

#[derive(Debug)]
struct PoolState<T> {
    /// A queue for unused entry keys.
    idle_keys: VecDeque<Key>,

    /// A queue for unused entries.
    idle_entries: VecDeque<EntryInner<T>>,

    /// A channel receiver for retrieving used entries.
    rx_used_entries: mpsc::Receiver<EntryInner<T>>,
}

impl<T> Pool<T> {
    /// Create a new `Pool` with the specified buffer size.
    pub fn new(bufsize: usize) -> Self {
        let (tx, rx) = mpsc::channel(bufsize);
        Self(Arc::new(PoolInner {
            bufsize,
            tx_used_entries: tx,
            state: Mutex::new(PoolState {
                idle_keys: (0..bufsize).collect(),
                idle_entries: VecDeque::with_capacity(bufsize),
                rx_used_entries: rx,
            }),
        }))
    }

    pub fn vacant_entry(&mut self) -> Option<VacantEntry<'_, T>> {
        let inner = Arc::get_mut(&mut self.0).expect("already shared");
        let state = inner.state.get_mut();
        let key = state.idle_keys.pop_front()?;
        Some(VacantEntry { key, state })
    }

    #[inline]
    fn new_entry(&self, inner: EntryInner<T>) -> PoolEntry<T> {
        PoolEntry {
            inner,
            pool: Arc::downgrade(&self.0),
        }
    }

    /// Acquire an entry.
    pub async fn acquire<F, E>(&self, factory: F) -> Result<PoolEntry<T>, E>
    where
        F: Fn() -> Result<T, E>,
    {
        let mut state = self.0.state.lock().await;

        // First, check if there is an entry available in `idle_values`.
        if let Some(entry) = state.idle_entries.pop_front() {
            return Ok(self.new_entry(entry));
        }

        // Check if any used entries have been returned to `rx_used`.
        // The check here is optimistic.
        match state.rx_used_entries.try_recv() {
            Ok(entry) => {
                return Ok(self.new_entry(entry));
            }
            Err(mpsc::error::TryRecvError::Empty) => {}
            Err(mpsc::error::TryRecvError::Closed) => unreachable!("channel is managed by pool"),
        }

        // Create a new entry using the factory function.
        // Each entry must contain a unique `Key`, and hence no new entry will be created
        // if `idle_keys` is exhausted.
        if let Some(key) = state.idle_keys.pop_front() {
            let value = factory()?;
            return Ok(self.new_entry(EntryInner { key, value }));
        }

        // Finally, if the entry keys are exhausted, block the current task
        // until an used entry is returned in `rx_used`.
        let entry = state.rx_used_entries.recv().await.unwrap();
        Ok(self.new_entry(entry))
    }
}

#[derive(Debug)]
pub struct VacantEntry<'a, T> {
    key: Key,
    state: &'a mut PoolState<T>,
}

impl<T> VacantEntry<'_, T> {
    pub fn insert(self, value: T) {
        self.state.idle_entries.push_back(EntryInner {
            key: self.key,
            value,
        });
    }
}

/// An acquired entry from a `Pool`.
#[derive(Debug)]
pub struct PoolEntry<T> {
    inner: EntryInner<T>,
    pool: Weak<PoolInner<T>>,
}

#[derive(Debug)]
struct EntryInner<T> {
    key: Key,
    value: T,
}

impl<T> Deref for PoolEntry<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner.value
    }
}

impl<T> DerefMut for PoolEntry<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner.value
    }
}

impl<T> PoolEntry<T> {
    /// Release the contained value into the pool.
    pub async fn release(this: Self) {
        let pool = this.pool.upgrade().expect("The pool is died");
        let mut tx_used = pool.tx_used_entries.clone();
        let _ = tx_used.send(this.inner).await;
    }
}
