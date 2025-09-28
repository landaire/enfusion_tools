use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use enfusion_pak::async_pak_vfs::AsyncReadAt;
use enfusion_pak::error::PakError;
use enfusion_pak::pak_vfs::ReadAt;
use enfusion_pak::vfs::VfsError;
use enfusion_pak::wrappers::bytes::BytesPakFileWrapper;
use memmap2::Mmap;

#[derive(Debug, Clone)]
#[repr(transparent)]
pub struct MmapWrapper(Arc<Mmap>);

#[async_trait]
impl AsyncReadAt for MmapWrapper {
    async fn read_at(
        &self,
        file_range: std::ops::Range<usize>,
    ) -> Result<impl AsRef<[u8]>, VfsError> {
        Ok(&self.0[file_range])
    }
}

impl ReadAt for MmapWrapper {
    fn read_at(&self, file_range: std::ops::Range<usize>) -> Result<impl AsRef<[u8]>, VfsError> {
        Ok(&self.0[file_range])
    }
}

impl AsRef<[u8]> for MmapWrapper {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

pub fn parse_pak_file(path: PathBuf) -> Result<BytesPakFileWrapper<MmapWrapper>, PakError> {
    let file = std::fs::File::open(&path)?;
    let mmap = unsafe { memmap2::Mmap::map(&file)? };

    let parsed_pak = enfusion_pak::PakFile::parse(&mmap)?;

    Ok(BytesPakFileWrapper::new(path, MmapWrapper(Arc::new(mmap)), parsed_pak))
}
