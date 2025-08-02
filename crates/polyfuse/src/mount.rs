pub mod unpriv;

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct MountOptions {
    options: Vec<String>,
    pub(crate) auto_unmount: bool,
    pub(crate) fusermount_path: Option<PathBuf>,
    fuse_comm_fd: Option<OsString>,
}

impl Default for MountOptions {
    fn default() -> Self {
        Self {
            options: vec![],
            auto_unmount: true,
            fusermount_path: None,
            fuse_comm_fd: None,
        }
    }
}

impl MountOptions {
    pub fn auto_unmount(&mut self, enabled: bool) -> &mut Self {
        self.auto_unmount = enabled;
        self
    }

    pub fn mount_option(&mut self, option: &str) -> &mut Self {
        for option in option.split(',').map(|s| s.trim()) {
            match option {
                "auto_unmount" => {
                    self.auto_unmount(true);
                }
                option => self.options.push(option.to_owned()),
            }
        }
        self
    }

    pub fn fusermount_path(&mut self, program: impl AsRef<OsStr>) -> &mut Self {
        let program = Path::new(program.as_ref());
        assert!(
            program.is_absolute(),
            "the binary path to `fusermount` must be absolute."
        );
        self.fusermount_path = Some(program.to_owned());
        self
    }

    pub fn fuse_comm_fd(&mut self, name: impl AsRef<OsStr>) -> &mut Self {
        self.fuse_comm_fd = Some(name.as_ref().to_owned());
        self
    }
}
