use std::{
    collections::VecDeque,
    ops::{Deref, DerefMut},
};
use tokio::sync::mpsc;

// FIXME: handling for invalidated entries.

type Key = usize;

/// Asynchronous object pool based on MPSC channel.
#[derive(Debug)]
pub struct Pool<T> {
    /// The length of internal buffers.
    bufsize: usize,

    /// A queue for unused entry keys.
    idle_keys: VecDeque<Key>,

    /// A queue for unused entries.
    idle_entries: VecDeque<EntryInner<T>>,

    /// A channel receiver for retrieving used entries.
    rx_used: mpsc::Receiver<EntryInner<T>>,

    /// A channel sender for returning used entries.
    tx_used: mpsc::Sender<EntryInner<T>>,
}

impl<T> Pool<T> {
    /// Create a new `Pool` with the specified buffer size.
    pub fn new(bufsize: usize) -> Self {
        let (tx_used, rx_used) = mpsc::channel(bufsize);
        Self {
            bufsize,
            idle_keys: (0..bufsize).collect(),
            idle_entries: VecDeque::with_capacity(bufsize),
            tx_used,
            rx_used,
        }
    }

    pub fn vacant_entry(&mut self) -> Option<VacantEntry<'_, T>> {
        let key = self.idle_keys.pop_front()?;
        Some(VacantEntry { key, pool: self })
    }

    #[inline]
    fn new_entry(&self, inner: EntryInner<T>) -> PoolEntry<T> {
        PoolEntry {
            inner,
            tx: self.tx_used.clone(),
        }
    }

    /// Acquire an entry.
    pub async fn acquire<F, E>(&mut self, factory: F) -> Result<PoolEntry<T>, E>
    where
        F: Fn() -> Result<T, E>,
    {
        // First, check if there is an entry available in the idle entries.
        if let Some(entry) = self.idle_entries.pop_front() {
            return Ok(self.new_entry(entry));
        }

        // Check if a entry have been returned to the pool channel.
        // The check here is optimistic: if the queue is empty, it will create a new
        // entry using the provided factory function, instead of blocking the current
        // task to wait for entries to be returned.
        // Since each entry must contain a unique `Key`, no new entry will be created
        // if `idle_keys` is exhausted.
        match self.rx_used.try_recv() {
            Ok(entry) => {
                return Ok(self.new_entry(entry));
            }
            Err(mpsc::error::TryRecvError::Empty) => {
                if let Some(key) = self.idle_keys.pop_front() {
                    let value = factory()?;
                    return Ok(self.new_entry(EntryInner { key, value }));
                }
            }
            Err(mpsc::error::TryRecvError::Closed) => unreachable!("channel is managed by pool"),
        }

        // Finally, if there are no entries available without blocking the current task,
        // wait until an existing entry is returned.
        let entry = self.rx_used.recv().await.unwrap();
        Ok(self.new_entry(entry))
    }
}

#[derive(Debug)]
pub struct VacantEntry<'a, T> {
    key: Key,
    pool: &'a mut Pool<T>,
}

impl<T> VacantEntry<'_, T> {
    pub fn insert(self, value: T) {
        self.pool.idle_entries.push_back(EntryInner {
            key: self.key,
            value,
        });
    }
}

/// An acquired entry from a `Pool`.
#[derive(Debug)]
pub struct PoolEntry<T> {
    inner: EntryInner<T>,
    tx: mpsc::Sender<EntryInner<T>>,
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
    pub async fn release(mut this: Self) {
        let _ = this.tx.send(this.inner).await;
    }
}
