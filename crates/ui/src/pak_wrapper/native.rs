use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use enfusion_pak::async_pak_vfs::AsyncReadAt;
use enfusion_pak::error::PakError;
use enfusion_pak::pak_vfs::ReadAt;
use enfusion_pak::vfs::VfsError;
use enfusion_pak::wrappers::bytes::BytesPakFileWrapper;
use memmap2::Mmap;

#[repr(transparent)]
#[derive(Clone, Debug)]
pub struct FileReference(pub std::path::PathBuf);

impl FileReference {
    pub fn has_supported_extension(&self) -> bool {
        matches!(self.0.extension().and_then(|e| e.to_str()), Some("pak" | "pbo"))
    }
}

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

#[async_trait]
impl dayz_pbo::async_pbo_vfs::AsyncReadAt for MmapWrapper {
    async fn read_at(
        &self,
        file_range: std::ops::Range<usize>,
    ) -> Result<impl AsRef<[u8]>, VfsError> {
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

/// Represents a parsed archive file — either a PAK or PBO — that can be
/// mounted as a VFS.
pub enum ParsedArchive {
    Pak(Arc<BytesPakFileWrapper<MmapWrapper>>),
    Pbo(dayz_pbo::pbo_vfs::PboVfs<MmapWrapper>),
}

pub fn parse_archive_file(path: PathBuf) -> Result<ParsedArchive, PakError> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();

    match ext.as_str() {
        "pbo" => {
            let file = std::fs::File::open(&path)?;
            let mmap = unsafe { memmap2::Mmap::map(&file)? };
            let pbo = dayz_pbo::PboFile::parse(&mmap).map_err(|e| {
                PakError::IoError(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    e.to_string(),
                ))
            })?;
            let vfs = dayz_pbo::pbo_vfs::PboVfs::new(MmapWrapper(Arc::new(mmap)), pbo);
            Ok(ParsedArchive::Pbo(vfs))
        }
        _ => {
            let pak = parse_pak_file(path)?;
            Ok(ParsedArchive::Pak(Arc::new(pak)))
        }
    }
}
