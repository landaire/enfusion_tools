use std::{
    fmt::Debug,
    io::Cursor,
};
use vfs::{FileSystem, VfsError, VfsMetadata, error::VfsErrorKind};

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
    T: std::ops::Deref,
    T::Target: AsRef<PakFile> + AsRef<[u8]>,
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
    T: std::ops::Deref + Sync + Send + Debug + 'static,
    T::Target: AsRef<PakFile> + AsRef<[u8]>,
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
        let entry = self.entry_at(path)?;
        let FileEntryMeta::File {
            offset,
            compressed_len,
            decompressed_len,
            compressed,
            
            ..
        } = entry.meta()
        else {
            return Err(VfsError::from(VfsErrorKind::NotSupported));
        };

        let mut data = Vec::with_capacity(*decompressed_len as usize);
        let data_start = *offset as usize;
        let data_end = data_start + *compressed_len as usize;

        let source_slice: &[u8] = self.source.as_ref();
        let mut source_range = &source_slice[data_start..data_end];
        if *compressed != 0 {
            let mut decoder = flate2::read::ZlibDecoder::new(source_range);
            std::io::copy(&mut decoder, &mut data).map_err(|err| {
                println!("error occurred during decompression: {:#?}", err);
                println!("offset: {:#X?}", *offset);
                VfsError::from(VfsErrorKind::IoError(err))
            })?;

            Ok(Box::new(Cursor::new(data)))
        } else {
            let _ = std::io::copy(&mut source_range, &mut data);

            Ok(Box::new(Cursor::new(data)))
        }
    }

    fn create_file(&self, path: &str) -> vfs::VfsResult<Box<dyn vfs::SeekAndWrite + Send>> {
        todo!()
    }

    fn append_file(&self, path: &str) -> vfs::VfsResult<Box<dyn vfs::SeekAndWrite + Send>> {
        todo!()
    }

    fn metadata(&self, path: &str) -> vfs::VfsResult<vfs::VfsMetadata> {
        let entry = self.entry_at(path)?;

        let pak_meta = entry.meta();
        let meta = match pak_meta {
            FileEntryMeta::Folder { children } => VfsMetadata {
                file_type: vfs::VfsFileType::Directory,
                len: 0,
                created: None,
                modified: None,
                accessed: None,
            },
            FileEntryMeta::File {
                offset,
                compressed_len,
                decompressed_len,
                unk,
                unk2,
                compressed,
                compression_level,
                timestamp,
            } => {
                // let converted_timestamp = pak_meta.parsed_timestamp().map(|ts| {
                //     SystemTime::fr
                // })
                VfsMetadata {
                    file_type: vfs::VfsFileType::File,
                    len: *decompressed_len as u64,
                    created: None,
                    modified: None,
                    accessed: None,
                }
            }
        };

        Ok(meta)
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
