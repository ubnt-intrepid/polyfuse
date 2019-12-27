use super::inode::{Directory, File, INode};
use crate::prelude::*;

use futures::lock::Mutex;
use polyfuse::FileAttr;
use std::{
    collections::hash_map::{Entry, HashMap},
    fs::Metadata,
    os::unix::fs::MetadataExt,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

pub struct INodeTable {
    inodes: Mutex<HashMap<u64, Arc<dyn INode>>>,
    next_id: AtomicU64,
}

impl INodeTable {
    pub fn new(metadata: &Metadata) -> Self {
        let mut attr = FileAttr::default();
        attr.set_ino(1);
        attr.set_nlink(2);
        attr.set_mode(metadata.mode());
        attr.set_uid(metadata.uid());
        attr.set_gid(metadata.gid());
        attr.set_atime_raw((metadata.atime() as u64, metadata.atime_nsec() as u32));
        attr.set_mtime_raw((metadata.mtime() as u64, metadata.mtime_nsec() as u32));
        attr.set_ctime_raw((metadata.ctime() as u64, metadata.ctime_nsec() as u32));

        let mut inodes = HashMap::new();
        inodes.insert(1, Arc::new(Directory::new(1, attr, None)) as Arc<dyn INode>);

        Self {
            inodes: Mutex::new(inodes),
            next_id: AtomicU64::new(2), // ino=1 is used by the root inode.
        }
    }

    pub async fn get(&self, ino: u64) -> Option<Arc<dyn INode>> {
        let inodes = self.inodes.lock().await;
        inodes.get(&ino).cloned()
    }

    pub async fn lookup(&self, parent: u64, name: &OsStr) -> Option<Arc<dyn INode>> {
        let parent = self.get(parent).await?;
        match parent.downcast_ref::<Directory>() {
            Some(parent) => parent.get(name).await?.upgrade(),
            None => None,
        }
    }

    pub async fn insert_file(
        &self,
        parent: u64,
        name: &OsStr,
        attr: FileAttr,
        data: Vec<u8>,
    ) -> Result<Arc<dyn INode>, libc::c_int> {
        let mut inodes = self.inodes.lock().await;

        let parent = inodes.get(&parent).ok_or_else(|| libc::ENOENT)?;
        let parent = parent
            .downcast_ref::<Directory>()
            .ok_or_else(|| libc::ENOTDIR)?;

        let ino = self.next_id.load(Ordering::SeqCst);
        let inode: Arc<dyn INode> = Arc::new(File::new(ino, attr, data));
        parent.insert(name, Arc::downgrade(&inode)).await?;
        inodes.insert(ino, inode.clone());
        self.next_id.fetch_add(1, Ordering::SeqCst);

        Ok(inode)
    }

    pub async fn insert_dir(
        &self,
        parent: u64,
        name: &OsStr,
        attr: FileAttr,
    ) -> Result<Arc<dyn INode>, libc::c_int> {
        let mut inodes = self.inodes.lock().await;

        let parent = inodes.get(&parent).ok_or_else(|| libc::ENOENT)?;
        let parent_handle = Arc::downgrade(parent);

        let parent = parent
            .downcast_ref::<Directory>()
            .ok_or_else(|| libc::ENOTDIR)?;

        let ino = self.next_id.load(Ordering::SeqCst);
        let inode: Arc<dyn INode> = Arc::new(Directory::new(ino, attr, Some(parent_handle)));
        parent.insert(name, Arc::downgrade(&inode)).await?;
        inodes.insert(ino, inode.clone());
        self.next_id.fetch_add(1, Ordering::SeqCst);

        Ok(inode)
    }

    pub async fn remove(&self, parent: u64, name: &OsStr) -> Result<(), libc::c_int> {
        let parent = self.get(parent).await.ok_or_else(|| libc::ENOENT)?;
        match parent.downcast_ref::<Directory>() {
            Some(parent) => {
                let inode = parent.remove(name).await?.upgrade().unwrap();

                let mut inodes = self.inodes.lock().await;
                if let Entry::Occupied(entry) = inodes.entry(inode.ino()) {
                    drop(entry.remove());
                }

                Ok(())
            }
            _ => Err(libc::ENOTDIR),
        }
    }
}
