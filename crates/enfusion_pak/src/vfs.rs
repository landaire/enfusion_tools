use std::fmt::Debug;
use vfs::{FileSystem, VfsError, error::VfsErrorKind};

use crate::{FileEntry, FileEntryMeta, PakFile};

#[derive(Debug)]
pub struct PakVfs<T> {
    source: T,
}

impl<T> PakVfs<T> {
    pub fn new(source: T) -> Self {
        Self { source }
    }
}

impl<T> PakVfs<T>
where
    T: AsRef<PakFile> + AsRef<[u8]>,
{
    pub fn entry_at(&self, path: &str) -> vfs::VfsResult<&FileEntry> {
        let pak: &PakFile = self.source.as_ref();
        let Some(crate::Chunk::File { fs }) = pak.file_chunk() else {
            panic!("failed to find file chunk")
        };

        let mut current: &FileEntry = fs;

        let path_parts = if path.starts_with("/") {
            path.split('/').skip(1)
        } else {
            #[allow(clippy::iter_skip_zero)]
            path.split('/').skip(0)
        };

        for part in path_parts {
            if part.is_empty() {
                continue;
            }

            let FileEntryMeta::Folder { children } = current.meta() else {
                return Err(VfsError::from(VfsErrorKind::NotSupported));
            };

            if let Some(next) = children.iter().find(|child| child.name() == part) {
                current = next;
            } else {
                return Err(VfsError::from(VfsErrorKind::FileNotFound));
            }
        }

        Ok(current)
    }
}

impl<T> FileSystem for PakVfs<T>
where
    T: AsRef<PakFile> + AsRef<[u8]> + Sync + Send + Debug + 'static,
{
    fn read_dir(&self, path: &str) -> vfs::VfsResult<Box<dyn Iterator<Item = String> + Send>> {
        let entry = self.entry_at(path)?;

        match entry.meta() {
            FileEntryMeta::Folder { children } => Ok(Box::new(
                children
                    .iter()
                    .map(|child| child.name().to_string())
                    .collect::<Vec<_>>()
                    .into_iter(),
            )),
            FileEntryMeta::File { .. } => Err(VfsError::from(VfsErrorKind::NotSupported)),
        }
    }

    fn create_dir(&self, path: &str) -> vfs::VfsResult<()> {
        todo!()
    }

    fn open_file(&self, path: &str) -> vfs::VfsResult<Box<dyn vfs::SeekAndRead + Send>> {
        todo!()
    }

    fn create_file(&self, path: &str) -> vfs::VfsResult<Box<dyn vfs::SeekAndWrite + Send>> {
        todo!()
    }

    fn append_file(&self, path: &str) -> vfs::VfsResult<Box<dyn vfs::SeekAndWrite + Send>> {
        todo!()
    }

    fn metadata(&self, path: &str) -> vfs::VfsResult<vfs::VfsMetadata> {
        todo!()
    }

    fn exists(&self, path: &str) -> vfs::VfsResult<bool> {
        if self.entry_at(path).is_ok() {
            return Ok(true);
        }

        Ok(false)
    }

    fn remove_file(&self, path: &str) -> vfs::VfsResult<()> {
        todo!()
    }

    fn remove_dir(&self, path: &str) -> vfs::VfsResult<()> {
        todo!()
    }

    fn set_creation_time(&self, _path: &str, _time: std::time::SystemTime) -> vfs::VfsResult<()> {
        Err(vfs::VfsError::from(vfs::error::VfsErrorKind::NotSupported))
    }

    fn set_modification_time(
        &self,
        _path: &str,
        _time: std::time::SystemTime,
    ) -> vfs::VfsResult<()> {
        Err(vfs::VfsError::from(vfs::error::VfsErrorKind::NotSupported))
    }

    fn set_access_time(&self, _path: &str, _time: std::time::SystemTime) -> vfs::VfsResult<()> {
        Err(vfs::VfsError::from(vfs::error::VfsErrorKind::NotSupported))
    }

    fn copy_file(&self, _src: &str, _dest: &str) -> vfs::VfsResult<()> {
        Err(vfs::error::VfsErrorKind::NotSupported.into())
    }

    fn move_file(&self, _src: &str, _dest: &str) -> vfs::VfsResult<()> {
        Err(vfs::error::VfsErrorKind::NotSupported.into())
    }

    fn move_dir(&self, _src: &str, _dest: &str) -> vfs::VfsResult<()> {
        Err(vfs::error::VfsErrorKind::NotSupported.into())
    }
}
