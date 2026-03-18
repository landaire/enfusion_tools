use async_trait::async_trait;
use std::fmt::Debug;
use std::ops::Range;
use vfs::VfsError;
use vfs::VfsMetadata;
use vfs::VfsResult;
use vfs::async_vfs::AsyncFileSystem;
use vfs::async_vfs::SeekAndRead;
use vfs::error::VfsErrorKind;

use crate::PakFile;
use crate::pak_vfs::PakVfs;

use futures::io::AsyncWrite;
use futures::io::Cursor;
use futures::stream::Stream;

/// Trait which allows for requesting a file be asynchronously read into memory.
#[async_trait]
pub trait AsyncPrime {
    /// Request the provided `file_range` be asynchronously primed and returned.
    async fn prime_file(&self, file_range: Range<usize>) -> Result<impl AsRef<[u8]>, VfsError>;
}

#[async_trait]
pub trait AsyncReadAt {
    /// Request the provided `file_range` be asynchronously primed and returned.
    async fn read_at(&self, file_range: Range<usize>) -> Result<impl AsRef<[u8]>, VfsError>;
}

/// Asynchronous VFS implementation for reading a `.pak` file.
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
        self.tree().async_vfs_read_dir(path)
    }

    async fn create_dir(&self, _path: &str) -> vfs::VfsResult<()> {
        Err(VfsErrorKind::NotSupported.into())
    }

    async fn open_file(&self, path: &str) -> VfsResult<Box<dyn SeekAndRead + Send + Unpin>> {
        let entry = self.tree().vfs_lookup(path)?;
        let fskit::VfsEntry::File(meta) = entry else {
            return Err(VfsError::from(VfsErrorKind::Other("not a file".into())));
        };

        let mut data = Vec::with_capacity(meta.decompressed_len as usize);
        let data_start = meta.offset as usize;
        let data_end = data_start + meta.compressed_len as usize;

        let primed_file = self.source.prime_file(data_start..data_end).await?;
        let source_slice: &[u8] = primed_file.as_ref();
        let mut source_range = source_slice;
        if meta.compressed != 0 {
            let mut decoder = flate2::read::ZlibDecoder::new(source_range);
            std::io::copy(&mut decoder, &mut data).map_err(|err| {
                println!("error occurred during decompression: {err:#?}");
                println!("offset: {:#X?}", meta.offset);
                VfsError::from(VfsErrorKind::IoError(err))
            })?;

            Ok(Box::new(Cursor::new(data)))
        } else {
            let _ = std::io::copy(&mut source_range, &mut data);

            Ok(Box::new(Cursor::new(data)))
        }
    }

    async fn create_file(&self, _path: &str) -> VfsResult<Box<dyn AsyncWrite + Send + Unpin>> {
        Err(VfsErrorKind::NotSupported.into())
    }

    async fn append_file(&self, _path: &str) -> VfsResult<Box<dyn AsyncWrite + Send + Unpin>> {
        Err(VfsErrorKind::NotSupported.into())
    }

    async fn metadata(&self, path: &str) -> vfs::VfsResult<VfsMetadata> {
        self.tree().vfs_metadata(path)
    }

    async fn exists(&self, path: &str) -> vfs::VfsResult<bool> {
        self.tree().vfs_exists(path)
    }

    async fn remove_file(&self, _path: &str) -> vfs::VfsResult<()> {
        Err(VfsErrorKind::NotSupported.into())
    }

    async fn remove_dir(&self, _path: &str) -> vfs::VfsResult<()> {
        Err(VfsErrorKind::NotSupported.into())
    }

    async fn set_creation_time(
        &self,
        _path: &str,
        _time: std::time::SystemTime,
    ) -> vfs::VfsResult<()> {
        Err(VfsErrorKind::NotSupported.into())
    }

    async fn set_modification_time(
        &self,
        _path: &str,
        _time: std::time::SystemTime,
    ) -> vfs::VfsResult<()> {
        Err(VfsErrorKind::NotSupported.into())
    }

    async fn set_access_time(
        &self,
        _path: &str,
        _time: std::time::SystemTime,
    ) -> vfs::VfsResult<()> {
        Err(VfsErrorKind::NotSupported.into())
    }

    async fn copy_file(&self, _src: &str, _dest: &str) -> vfs::VfsResult<()> {
        Err(VfsErrorKind::NotSupported.into())
    }

    async fn move_file(&self, _src: &str, _dest: &str) -> vfs::VfsResult<()> {
        Err(VfsErrorKind::NotSupported.into())
    }

    async fn move_dir(&self, _src: &str, _dest: &str) -> vfs::VfsResult<()> {
        Err(VfsErrorKind::NotSupported.into())
    }
}
