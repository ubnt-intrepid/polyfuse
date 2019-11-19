use crate::prelude::*;
use polyfuse::{DirEntry, FileAttr};

use crossbeam::atomic::AtomicCell;
use futures::lock::Mutex;
use std::{
    collections::hash_map::{Entry, HashMap},
    io,
    os::linux::fs::MetadataExt,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Weak,
    },
};

pub struct INodeTable {
    inodes: Mutex<HashMap<u64, Arc<INode>>>,
    next_id: AtomicU64,
}

impl INodeTable {
    pub fn new(mountpoint: impl AsRef<Path>) -> io::Result<Self> {
        let metadata = std::fs::metadata(mountpoint)?;
        if !metadata.is_dir() {
            return Err(io::Error::from_raw_os_error(libc::ENOTDIR));
        }

        let mut attr = FileAttr::default();
        attr.set_ino(1);
        attr.set_nlink(2);
        attr.set_mode(metadata.st_mode());
        attr.set_uid(metadata.st_uid());
        attr.set_gid(metadata.st_gid());
        attr.set_atime(metadata.st_atime() as u64, metadata.st_atime_nsec() as u32);
        attr.set_mtime(metadata.st_mtime() as u64, metadata.st_mtime_nsec() as u32);
        attr.set_ctime(metadata.st_ctime() as u64, metadata.st_ctime_nsec() as u32);

        let mut inodes = HashMap::new();
        inodes.insert(
            1,
            Arc::new(INode {
                ino: 1,
                kind: INodeKind::Dir(Directory {
                    ino: 1,
                    attr: AtomicCell::new(attr),
                    parent: None,
                    children: Mutex::new(HashMap::new()),
                }),
            }),
        );

        Ok(Self {
            inodes: Mutex::new(inodes),
            next_id: AtomicU64::new(2), // ino=1 is used by the root inode.
        })
    }

    pub async fn lookup(&self, parent: u64, name: &OsStr) -> Option<Arc<INode>> {
        tracing::debug!("lookup({:?}, {:?})", parent, name);
        let inodes = self.inodes.lock().await;
        let parent = inodes.get(&parent)?;
        match &parent.kind {
            INodeKind::Dir(dir) => dir.get_child(name).await,
            _ => None,
        }
    }

    pub async fn get(&self, ino: u64) -> Option<Arc<INode>> {
        let inodes = self.inodes.lock().await;
        inodes.get(&ino).cloned()
    }

    pub async fn insert_file(
        &self,
        parent: u64,
        name: &OsStr,
        mut attr: FileAttr,
        data: Vec<u8>,
    ) -> Result<FileAttr, libc::c_int> {
        let mut inodes = self.inodes.lock().await;

        let parent = inodes.get(&parent).ok_or_else(|| libc::ENOENT)?;
        match &parent.kind {
            INodeKind::Dir(parent) => {
                let ino = self.next_id.load(Ordering::SeqCst);
                attr.set_ino(ino);
                attr.set_mode(attr.mode() | libc::S_IFREG);
                attr.set_size(data.len() as u64);

                let inode = Arc::new(INode {
                    ino,
                    kind: INodeKind::File(File {
                        attr: AtomicCell::new(attr),
                        data: Mutex::new(data),
                    }),
                });

                parent.insert_child(name, Arc::downgrade(&inode)).await?;
                inodes.insert(ino, inode);

                self.next_id.fetch_add(1, Ordering::SeqCst);
                Ok(attr)
            }
            _ => Err(libc::ENOTDIR),
        }
    }

    pub async fn insert_dir(
        &self,
        parent: u64,
        name: &OsStr,
        mut attr: FileAttr,
    ) -> Result<FileAttr, libc::c_int> {
        tracing::debug!("insert_dir");

        let mut inodes = self.inodes.lock().await;

        let parent = inodes.get(&parent).ok_or_else(|| libc::ENOENT)?;
        let parent_handle = Arc::downgrade(parent);

        match &parent.kind {
            INodeKind::Dir(parent) => {
                let ino = self.next_id.load(Ordering::SeqCst);
                attr.set_ino(ino);
                attr.set_mode(attr.mode() | libc::S_IFDIR);

                let inode = Arc::new(INode {
                    ino,
                    kind: INodeKind::Dir(Directory {
                        ino,
                        attr: AtomicCell::new(attr),
                        parent: Some(parent_handle),
                        children: Mutex::new(HashMap::new()),
                    }),
                });
                let inode_handle = Arc::downgrade(&inode);

                parent.insert_child(name, inode_handle).await?;
                inodes.insert(ino, inode);

                self.next_id.fetch_add(1, Ordering::SeqCst);
                Ok(attr)
            }
            _ => Err(libc::ENOTDIR),
        }
    }

    pub async fn remove(&self, parent: u64, name: &OsStr) -> Result<(), libc::c_int> {
        let parent = self.get(parent).await.ok_or_else(|| libc::ENOENT)?;
        match &parent.kind {
            INodeKind::Dir(dir) => {
                let mut children = dir.children.lock().await;
                match children.entry(name.into()) {
                    Entry::Occupied(entry) => {
                        let inode = entry.get().upgrade().unwrap();
                        if !inode.is_removable().await {
                            return Err(libc::EACCES);
                        }
                        entry.remove();

                        let mut inodes = self.inodes.lock().await;
                        if let Entry::Occupied(entry) = inodes.entry(inode.ino) {
                            drop(entry.remove());
                        }

                        Ok(())
                    }
                    Entry::Vacant(..) => Err(libc::ENOENT),
                }
            }
            _ => Err(libc::ENOTDIR),
        }
    }
}

pub struct INode {
    ino: u64,
    kind: INodeKind,
}

enum INodeKind {
    Dir(Directory),
    File(File),
}

impl INode {
    pub fn load_attr(&self) -> FileAttr {
        match &self.kind {
            INodeKind::Dir(dir) => dir.attr.load(),
            INodeKind::File(file) => file.attr.load(),
        }
    }

    pub fn store_attr(&self, attr: FileAttr) {
        match &self.kind {
            INodeKind::Dir(dir) => dir.attr.store(attr),
            INodeKind::File(file) => file.attr.store(attr),
        }
    }

    pub fn as_file(&self) -> Option<&File> {
        match self.kind {
            INodeKind::File(ref file) => Some(file),
            _ => None,
        }
    }

    pub fn as_dir(&self) -> Option<&Directory> {
        match self.kind {
            INodeKind::Dir(ref dir) => Some(dir),
            _ => None,
        }
    }

    pub async fn is_removable(&self) -> bool {
        match self.kind {
            INodeKind::Dir(ref dir) => dir.children.lock().await.is_empty(),
            _ => true,
        }
    }
}

pub struct Directory {
    ino: u64,
    attr: AtomicCell<FileAttr>,
    parent: Option<Weak<INode>>,
    children: Mutex<HashMap<OsString, Weak<INode>>>,
}

impl Directory {
    pub async fn entries(&self) -> Vec<DirEntry> {
        let mut entries = vec![DirEntry::dir(".", self.ino, 1)];
        let mut offset = 2;

        if let Some(ref parent) = self.parent {
            let parent = parent.upgrade().unwrap();
            entries.push(DirEntry::dir("..", parent.ino, 2));
            offset += 1;
        }

        let children = self.children.lock().await;
        entries.extend(children.iter().enumerate().map(|(i, (name, inode))| {
            let ino = inode.upgrade().unwrap().ino;
            DirEntry::new(name, ino, i as u64 + offset)
        }));

        entries
    }

    async fn get_child(&self, name: &OsStr) -> Option<Arc<INode>> {
        let children = self.children.lock().await;
        children.get(name)?.upgrade()
    }

    async fn insert_child(&self, name: &OsStr, inode: Weak<INode>) -> Result<(), libc::c_int> {
        let mut children = self.children.lock().await;
        match children.entry(name.into()) {
            Entry::Occupied(..) => Err(libc::EEXIST),
            Entry::Vacant(entry) => {
                entry.insert(inode);
                Ok(())
            }
        }
    }
}

pub struct File {
    attr: AtomicCell<FileAttr>,
    data: Mutex<Vec<u8>>,
}

impl File {
    pub async fn read(&self, offset: usize, bufsize: usize) -> Option<Vec<u8>> {
        let data = self.data.lock().await;

        if offset >= data.len() {
            return None;
        }

        let data = &data[offset..];
        Some(data[..std::cmp::min(data.len(), bufsize)].to_vec())
    }

    pub async fn write(&self, offset: usize, data: &[u8]) {
        let mut orig_data = self.data.lock().await;

        orig_data.resize(offset + data.len(), 0);

        let out = &mut orig_data[offset..offset + data.len()];
        out.copy_from_slice(data);
    }
}
