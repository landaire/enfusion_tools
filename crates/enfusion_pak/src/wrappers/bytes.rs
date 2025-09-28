use std::fmt::Debug;
use std::path::Path;
use std::path::PathBuf;

#[cfg(feature = "async_vfs")]
use async_trait::async_trait;
use vfs::VfsError;

use crate::PakFile;
#[cfg(feature = "async_vfs")]
use crate::async_pak_vfs::AsyncPrime;
use crate::pak_vfs::Prime;

/// A [`PakFile`] wrapper which retains the source path from some bytes source (e.g. mmap'd file)
#[allow(unused)]
pub struct BytesPakFileWrapper<T> {
    path: PathBuf,
    source: T,
    pak_file: PakFile,
}

impl<T> BytesPakFileWrapper<T> {
    pub fn new(path: PathBuf, source: T, pak_file: PakFile) -> Self {
        Self { path, source, pak_file }
    }

    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn pak_file(&self) -> &PakFile {
        &self.pak_file
    }

    pub fn pak_file_mut(&mut self) -> &mut PakFile {
        &mut self.pak_file
    }

    pub fn source(&self) -> &T {
        &self.source
    }
}

impl<T> BytesPakFileWrapper<T>
where
    T: AsRef<[u8]>,
{
    fn source_bytes(&self) -> &[u8] {
        self.source.as_ref()
    }
}

impl<T> Debug for BytesPakFileWrapper<T>
where
    T: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BytesPakFileWrapper")
            .field("path", &self.path)
            .field("source", &self.source)
            .field("pak_file", &self.pak_file)
            .finish()
    }
}

impl<T> AsRef<PakFile> for BytesPakFileWrapper<T> {
    fn as_ref(&self) -> &PakFile {
        &self.pak_file
    }
}

impl<T> Prime for BytesPakFileWrapper<T>
where
    T: AsRef<[u8]>,
{
    fn prime_file(&self, file_range: std::ops::Range<usize>) -> Result<impl AsRef<[u8]>, VfsError> {
        Ok(&self.source_bytes()[file_range])
    }
}

#[cfg(feature = "async_vfs")]
#[async_trait]
impl<T> AsyncPrime for BytesPakFileWrapper<T>
where
    T: AsRef<[u8]> + Sync,
{
    async fn prime_file(
        &self,
        file_range: std::ops::Range<usize>,
    ) -> Result<impl AsRef<[u8]>, VfsError> {
        Ok(&self.source_bytes()[file_range])
    }
}
