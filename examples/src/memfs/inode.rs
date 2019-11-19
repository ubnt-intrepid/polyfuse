use crate::prelude::*;
use polyfuse::{DirEntry, FileAttr};

use crossbeam::atomic::AtomicCell;
use futures::lock::Mutex;
use std::{
    any::TypeId,
    collections::hash_map::{Entry, HashMap},
    sync::Weak,
};

pub trait INode: Send + Sync + 'static {
    fn ino(&self) -> u64;
    fn load_attr(&self) -> FileAttr;
    fn store_attr(&self, attr: FileAttr);

    #[doc(hidden)] // private API
    fn __private_type_id__(&self) -> TypeId {
        TypeId::of::<Self>()
    }
}

impl dyn INode {
    pub fn downcast_ref<T: INode>(&self) -> Option<&T> {
        if self.__private_type_id__() == TypeId::of::<T>() {
            Some(unsafe { &*(self as *const dyn INode as *const T) })
        } else {
            None
        }
    }
}

pub struct Directory {
    ino: u64,
    attr: AtomicCell<FileAttr>,
    parent: Option<Weak<dyn INode>>,
    children: Mutex<HashMap<OsString, Weak<dyn INode>>>,
}

impl INode for Directory {
    fn ino(&self) -> u64 {
        self.ino
    }

    fn load_attr(&self) -> FileAttr {
        self.attr.load()
    }

    fn store_attr(&self, attr: FileAttr) {
        self.attr.store(attr);
    }
}

impl Directory {
    pub fn new(ino: u64, mut attr: FileAttr, parent: Option<Weak<dyn INode>>) -> Self {
        attr.set_ino(ino);
        attr.set_mode((attr.mode() & libc::S_IFMT) | libc::S_IFDIR);

        Self {
            ino,
            attr: AtomicCell::new(attr),
            parent,
            children: Mutex::new(HashMap::new()),
        }
    }

    pub async fn is_empty(&self) -> bool {
        self.children.lock().await.is_empty()
    }

    pub async fn get(&self, name: &OsStr) -> Option<Weak<dyn INode>> {
        self.children.lock().await.get(name).cloned()
    }

    pub async fn insert(&self, name: &OsStr, inode: Weak<dyn INode>) -> Result<(), libc::c_int> {
        let mut children = self.children.lock().await;
        match children.entry(name.into()) {
            Entry::Occupied(..) => Err(libc::EEXIST),
            Entry::Vacant(entry) => {
                entry.insert(inode);
                Ok(())
            }
        }
    }

    pub async fn remove(&self, name: &OsStr) -> Result<Weak<dyn INode>, libc::c_int> {
        let mut children = self.children.lock().await;
        match children.entry(name.into()) {
            Entry::Occupied(entry) => {
                let inode = entry.get().upgrade().unwrap();
                if let Some(dir) = inode.downcast_ref::<Directory>() {
                    if !dir.is_empty().await {
                        return Err(libc::ENOTEMPTY);
                    }
                }

                Ok(entry.remove())
            }
            Entry::Vacant(..) => Err(libc::ENOENT),
        }
    }

    pub async fn entries(&self) -> Vec<DirEntry> {
        let mut entries = vec![DirEntry::dir(".", self.ino, 1)];
        let mut offset = 2;

        if let Some(ref parent) = self.parent {
            let parent = parent.upgrade().unwrap();
            entries.push(DirEntry::dir("..", parent.ino(), 2));
            offset += 1;
        }

        let children = self.children.lock().await;
        entries.extend(children.iter().enumerate().map(|(i, (name, inode))| {
            let ino = inode.upgrade().unwrap().ino();
            DirEntry::new(name, ino, i as u64 + offset)
        }));

        entries
    }
}

#[derive(Debug)]
pub struct File {
    ino: u64,
    attr: AtomicCell<FileAttr>,
    data: Mutex<Vec<u8>>,
}

impl INode for File {
    fn ino(&self) -> u64 {
        self.ino
    }

    fn load_attr(&self) -> FileAttr {
        self.attr.load()
    }

    fn store_attr(&self, attr: FileAttr) {
        self.attr.store(attr);
    }
}

impl File {
    pub fn new(ino: u64, mut attr: FileAttr, data: Vec<u8>) -> Self {
        attr.set_ino(ino);
        attr.set_mode((attr.mode() & libc::S_IFMT) | libc::S_IFREG);
        attr.set_size(data.len() as u64);

        Self {
            ino,
            attr: AtomicCell::new(attr),
            data: Mutex::new(data),
        }
    }

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
