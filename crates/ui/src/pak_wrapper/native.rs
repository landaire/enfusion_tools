use std::path::PathBuf;

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

impl AsRef<[u8]> for WrappedPakFile {
    fn as_ref(&self) -> &[u8] {
        &self.source
    }
}

pub fn parse_pak_file(path: PathBuf) -> Result<WrappedPakFile, PakError> {
    let file = std::fs::File::open(&path)?;
    let mmap = unsafe { memmap2::Mmap::map(&file)? };

    let parsed_pak = enfusion_pak::PakFile::parse(&mmap)?;

    Ok(WrappedPakFile { path, source: mmap, pak_file: parsed_pak })
}
