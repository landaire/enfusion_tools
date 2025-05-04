use async_trait::async_trait;
use std::{fmt::Debug, ops::Range};
use vfs::{
    VfsError, VfsMetadata, VfsResult,
    async_vfs::{AsyncFileSystem, SeekAndRead},
    error::VfsErrorKind,
};

use crate::{FileEntryMeta, PakFile, pak_vfs::PakVfs};

use async_std::{
    io::{Cursor, Write},
    stream::{self, Stream},
};

#[async_trait]
pub trait AsyncPrime {
    async fn prime_file(&self, file_range: Range<usize>) -> impl AsRef<[u8]>;
}

#[async_trait]
impl<T> AsyncFileSystem for PakVfs<T>
where
    T: std::ops::Deref + Sync + Send + Debug + 'static,
    T::Target: AsRef<PakFile> + AsyncPrime,
{
    async fn read_dir(
        &self,
        path: &str,
    ) -> VfsResult<Box<dyn Unpin + Stream<Item = String> + Send>> {
        let entry = self.entry_at(path)?;

        match entry.meta() {
            FileEntryMeta::Folder { children } => Ok(Box::new(stream::from_iter(
                children
                    .iter()
                    .map(|child| child.name().to_string())
                    .collect::<Vec<_>>()
                    .into_iter(),
            ))),
            FileEntryMeta::File { .. } => Err(VfsError::from(VfsErrorKind::NotSupported)),
        }
    }

    async fn create_dir(&self, path: &str) -> vfs::VfsResult<()> {
        todo!()
    }

    async fn open_file(&self, path: &str) -> VfsResult<Box<dyn SeekAndRead + Send + Unpin>> {
        let entry = self.entry_at(path)?;
        let FileEntryMeta::File { offset, compressed_len, decompressed_len, compressed, .. } =
            entry.meta()
        else {
            return Err(VfsError::from(VfsErrorKind::NotSupported));
        };

        let mut data = Vec::with_capacity(*decompressed_len as usize);
        let data_start = *offset as usize;
        let data_end = data_start + *compressed_len as usize;

        let primed_file = self.source.prime_file(data_start..data_end).await;
        let source_slice: &[u8] = primed_file.as_ref();
        let mut source_range = source_slice;
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

    async fn create_file(&self, path: &str) -> VfsResult<Box<dyn Write + Send + Unpin>> {
        todo!()
    }

    async fn append_file(&self, path: &str) -> VfsResult<Box<dyn Write + Send + Unpin>> {
        todo!()
    }

    async fn metadata(&self, path: &str) -> vfs::VfsResult<vfs::VfsMetadata> {
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

    async fn exists(&self, path: &str) -> vfs::VfsResult<bool> {
        if self.entry_at(path).is_ok() {
            return Ok(true);
        }

        Ok(false)
    }

    async fn remove_file(&self, path: &str) -> vfs::VfsResult<()> {
        todo!()
    }

    async fn remove_dir(&self, path: &str) -> vfs::VfsResult<()> {
        todo!()
    }

    async fn set_creation_time(
        &self,
        _path: &str,
        _time: std::time::SystemTime,
    ) -> vfs::VfsResult<()> {
        Err(vfs::VfsError::from(vfs::error::VfsErrorKind::NotSupported))
    }

    async fn set_modification_time(
        &self,
        _path: &str,
        _time: std::time::SystemTime,
    ) -> vfs::VfsResult<()> {
        Err(vfs::VfsError::from(vfs::error::VfsErrorKind::NotSupported))
    }

    async fn set_access_time(
        &self,
        _path: &str,
        _time: std::time::SystemTime,
    ) -> vfs::VfsResult<()> {
        Err(vfs::VfsError::from(vfs::error::VfsErrorKind::NotSupported))
    }

    async fn copy_file(&self, _src: &str, _dest: &str) -> vfs::VfsResult<()> {
        Err(vfs::error::VfsErrorKind::NotSupported.into())
    }

    async fn move_file(&self, _src: &str, _dest: &str) -> vfs::VfsResult<()> {
        Err(vfs::error::VfsErrorKind::NotSupported.into())
    }

    async fn move_dir(&self, _src: &str, _dest: &str) -> vfs::VfsResult<()> {
        Err(vfs::error::VfsErrorKind::NotSupported.into())
    }
}
