use std::{io::Seek, path::PathBuf};

use enfusion_pak::PakFile;

const BUFFER_SIZE_BYTES: usize = 1024 * 1024 * 20;

#[derive(Debug)]
pub struct WrappedPakFile {
    path: PathBuf,
    handle: rfd::FileHandle,
    buffer: oval::Buffer,
    pak_file: PakFile,
    pos: usize,
}

impl AsRef<PakFile> for WrappedPakFile {
    fn as_ref(&self) -> &PakFile {
        &self.pak_file
    }
}

impl AsRef<[u8]> for WrappedPakFile {
    fn as_ref(&self) -> &[u8] {
        &self.buffer.data()
    }
}

pub fn parse_pak_file(path: PathBuf) -> Result<WrappedPakFile, PakError> {
    let file = std::fs::File::open(&path)?;
    let mmap = unsafe { memmap2::Mmap::map(&file)? };

    let parsed_pak = enfusion_pak::PakFile::parse(&mmap)?;

    Ok(WrappedPakFile { path, source: mmap, pak_file: parsed_pak })
}
