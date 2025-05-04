use std::path::PathBuf;

use async_trait::async_trait;
use enfusion_pak::async_pak_vfs::AsyncPrime;
use enfusion_pak::pak_vfs::Prime;
use enfusion_pak::{PakFile, error::PakError};
use memmap2::Mmap;

#[derive(Debug)]
pub struct WrappedPakFile {
    path: PathBuf,
    source: Mmap,
    pak_file: PakFile,
}

impl AsRef<PakFile> for WrappedPakFile {
    fn as_ref(&self) -> &PakFile {
        &self.pak_file
    }
}

impl Prime for WrappedPakFile {
    fn prime_file(&self, file_range: std::ops::Range<usize>) -> impl AsRef<[u8]> {
        &self.source[file_range]
    }
}

#[async_trait]
impl AsyncPrime for WrappedPakFile {
    async fn prime_file(&self, file_range: std::ops::Range<usize>) -> impl AsRef<[u8]> {
        &self.source[file_range]
    }
}

pub fn parse_pak_file(path: PathBuf) -> Result<WrappedPakFile, PakError> {
    let file = std::fs::File::open(&path)?;
    let mmap = unsafe { memmap2::Mmap::map(&file)? };

    let parsed_pak = enfusion_pak::PakFile::parse(&mmap)?;

    Ok(WrappedPakFile { path, source: mmap, pak_file: parsed_pak })
}
