use std::ops::Range;

use crate::error::PakError;
use jiff::civil::DateTime;
use kinded::{Kind, Kinded};
use variantly::Variantly;
use winnow::binary::{be_u32, le_u16, le_u32, u8};
use winnow::stream::Location;
use winnow::token::take;
use winnow::{LocatingSlice, Parser, Partial, Result as WResult};

#[derive(Debug, Clone)]
pub struct FileEntry {
    name: String,
    meta: FileEntryMeta,
}

impl FileEntry {
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    pub fn kind(&self) -> FileEntryKind {
        self.meta.kind()
    }

    pub fn meta(&self) -> &FileEntryMeta {
        &self.meta
    }

    pub fn merge(&mut self, other: Self) {
        let FileEntryMeta::Folder {
            children: self_children,
        } = &mut self.meta
        else {
            panic!("merge should only be called on directories");
        };

        let FileEntryMeta::Folder {
            children: other_children,
        } = other.meta
        else {
            panic!("merge should only be called on directories");
        };

        for other_child in other_children {
            if let Some(self_child) = self_children
                .iter_mut()
                .find(|self_child| self_child.name == other_child.name)
            {
                if other_child.kind() == FileEntryKind::File {
                    println!("{:#?}, {:#?}", self_child, other_child);
                }
                assert_eq!(other_child.kind(), self_child.kind());
                assert_ne!(
                    other_child.kind(),
                    FileEntryKind::File,
                    "File was duplicated across PAK files"
                );
                self_child.merge(other_child);
            } else {
                self_children.push(other_child);
            }
        }
    }
}

#[derive(Debug, Clone, Kinded, Variantly)]
#[kinded(kind = FileEntryKind)]
pub enum FileEntryMeta {
    Folder {
        children: Vec<FileEntry>,
    },
    File {
        offset: u32,
        compressed_len: u32,
        decompressed_len: u32,
        unk: u32,
        unk2: u16,
        compressed: u8,
        compression_level: u8,
        timestamp: u32,
    },
}

impl FileEntryMeta {
    /// Adds a child to this file entry. No-op if this is a folder
    pub fn push_child(&mut self, child: FileEntry) {
        if let FileEntryMeta::Folder { children } = self {
            children.push(child);
        }
    }

    pub fn parsed_timestamp(&self) -> Option<jiff::civil::DateTime> {
        match self {
            FileEntryMeta::Folder { .. } => None,
            FileEntryMeta::File { timestamp, .. } => {
                let year = (timestamp >> 26) + 2000;
                let month = (timestamp >> 22) & 0xf;
                let day = (timestamp >> 17) & 0x1f;
                let hour = (timestamp >> 12) & 0x1f;
                let minute = (timestamp >> 6) & 0x3f;
                let second = timestamp & 0x3f;

                DateTime::new(
                    year as i16,
                    month as i8,
                    day as i8,
                    hour as i8,
                    minute as i8,
                    second as i8,
                    0,
                )
                .ok()
            }
        }
    }
}

impl TryFrom<u8> for FileEntryKind {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        let result = match value {
            0 => Self::Folder,
            1 => Self::File,
            _ => {
                panic!("unknown file entry kind: {:#X}", value);
            }
        };

        Ok(result)
    }
}

#[derive(Debug)]
pub struct PakFile {
    chunks: Vec<Chunk>,
}

impl PakFile {
    pub fn chunks(&self) -> &[Chunk] {
        &self.chunks
    }

    pub fn chunks_mut(&mut self) -> &mut Vec<Chunk> {
        &mut self.chunks
    }

    pub fn file_chunk(&self) -> Option<&Chunk> {
        self.chunks.iter().find(|chunk| chunk.is_file())
    }

    pub fn file_chunk_mut(&mut self) -> Option<&mut Chunk> {
        self.chunks.iter_mut().find(|chunk| chunk.is_file())
    }
}

#[derive(Debug)]
pub enum PakType {
    PAC1,
}

#[derive(Debug, Kinded, Variantly)]
pub enum Chunk {
    Form {
        file_size: u32,
        pak_file_type: PakType,
    },
    Head {
        version: u32,
        header_data: Range<usize>,
    },
    Data {
        data: Range<usize>,
    },
    File {
        fs: FileEntry,
    },
    Unknown(u32),
}

impl PakFile {
    pub fn parse(input: &[u8]) -> Result<PakFile, PakError> {
        parse_pak(input)
    }
}

#[derive(Default)]
struct ParserContext {
    offset: usize,
}

fn seek(ahead: usize, input: &mut LocatingSlice<Partial<&[u8]>>) -> WResult<()> {
    take(ahead).void().parse_next(input)
}

fn parse_pak(input: &[u8]) -> Result<PakFile, PakError> {
    let mut chunks = vec![];

    let mut input = LocatingSlice::new(Partial::new(input));

    loop {
        match parse_chunk(&mut input) {
            Ok((skip_bytes, chunk)) => {
                seek(skip_bytes, &mut input);
                chunks.push(chunk);
            }
            Err(e) => {
                println!("{:?}", e);
                break;
            }
        }
    }

    Ok(PakFile { chunks })
}

fn parse_form_chunk(input: &mut LocatingSlice<Partial<&[u8]>>) -> WResult<(usize, Chunk)> {
    let file_size = be_u32(input)?;
    let pak_type_bytes: [u8; 4] = take(4usize)
        .parse_next(input)?
        .try_into()
        .expect("winnow should have returned a 4-byte buffer");
    let pak_file_type = match &pak_type_bytes {
        b"PAC1" => PakType::PAC1,
        unk => {
            panic!("unknown pak type: {:?}", unk);
        }
    };
    Ok((
        0,
        Chunk::Form {
            file_size,
            pak_file_type,
        },
    ))
}

fn parse_head_chunk(input: &mut LocatingSlice<Partial<&[u8]>>) -> WResult<(usize, Chunk)> {
    let header_len = be_u32.parse_next(input)? as usize;
    assert_eq!(header_len, 0x1c);

    let mut skip_bytes = 0;

    let header_start = input.current_token_start();
    let version = le_u32.parse_next(input)?;
    skip_bytes += 4;

    let header_range = (header_start + skip_bytes)..(header_start + header_len);

    let chunk = Chunk::Head {
        version,
        header_data: header_range,
    };

    Ok((header_len - skip_bytes, chunk))
}

fn parse_data_chunk(input: &mut LocatingSlice<Partial<&[u8]>>) -> WResult<(usize, Chunk)> {
    let data_len = be_u32.parse_next(input)? as usize;

    let data_start = input.current_token_start();
    //take(data_len).void().parse_next(input)?;

    let chunk = Chunk::Data {
        data: data_start..(data_start + data_len),
    };

    Ok((data_len, chunk))
}

fn parse_file_entry(input: &mut &[u8]) -> WResult<(FileEntry, usize)> {
    let entry_kind: FileEntryKind = u8(input)?.try_into().expect("???");
    let name_len = u8(input)?;
    let name = take(name_len).parse_next(input)?;
    let name =
        String::from_utf8(name.to_vec()).expect("name does not contain valid UTF8 characters");

    let (meta, children) = match entry_kind {
        FileEntryKind::Folder => {
            let children_count = le_u32(input)?;
            (
                FileEntryMeta::Folder {
                    children: Default::default(),
                },
                children_count as usize,
            )
        }
        FileEntryKind::File => {
            let offset = le_u32(input)?;
            let compressed_len = le_u32(input)?;
            let decompressed_len = le_u32(input)?;
            let unknown = le_u32(input)?;
            let unk2 = le_u16(input)?;
            let compressed = u8(input)?;
            let compression_level = u8(input)?;
            let timestamp = le_u32(input)?;

            assert_eq!(unknown, 0);
            assert_eq!(unk2, 0);
            assert!(matches!(compressed, 0 | 1));
            assert!(matches!(compression_level, 0 | 6));

            (
                FileEntryMeta::File {
                    offset,
                    compressed_len,
                    decompressed_len,
                    unk: unknown,
                    unk2,
                    compressed,
                    compression_level,
                    timestamp,
                },
                0,
            )
        }
    };

    Ok((FileEntry { name, meta }, children))
}

fn parse_file_chunk(input: &mut LocatingSlice<Partial<&[u8]>>) -> WResult<(usize, Chunk)> {
    let chunk_len = be_u32(input)?;

    let mut chunk_data = take(chunk_len).parse_next(input)?;

    struct Directory {
        is_root: bool,
        children_remaining: usize,
        entry: FileEntry,
    }

    let mut parents = vec![];
    let mut parsed_root = false;

    while !chunk_data.is_empty() {
        let (entry, children) = parse_file_entry(&mut chunk_data)?;

        match entry.meta.kind() {
            FileEntryKind::Folder => {
                parents.push(Directory {
                    is_root: !parsed_root,
                    children_remaining: children,
                    entry,
                });

                parsed_root = true;
            }
            FileEntryKind::File => {
                let parent = parents.last_mut().expect("bug: no parent for this file");
                parent.children_remaining = parent
                    .children_remaining
                    .checked_sub(1)
                    .expect("encountered more children than expected for a folder");

                parent.entry.meta.push_child(entry);
            }
        }

        // Check to see if the parents can be coalesced
        while let Some(dir) =
            parents.pop_if(|parent| parent.children_remaining == 0 && !parent.is_root)
        {
            let parent = parents
                .last_mut()
                .expect("expected a folder to have a parent, but there is none");

            parent.children_remaining = parent
                .children_remaining
                .checked_sub(1)
                .expect("encountered more children than expected for a folder");

            parent.entry.meta.push_child(dir.entry);
        }
    }

    assert_eq!(parents.len(), 1);

    let chunk = Chunk::File {
        fs: parents.pop().expect("no parents?").entry,
    };

    Ok((0, chunk))
}

fn parse_chunk(input: &mut LocatingSlice<Partial<&[u8]>>) -> WResult<(usize, Chunk)> {
    let fourcc: [u8; 4] = take(4usize)
        .parse_next(input)?
        .try_into()
        .expect("winnow should have returned a 4-byte buffer");

    match &fourcc {
        b"FORM" => parse_form_chunk(input),
        b"HEAD" => parse_head_chunk(input),
        b"DATA" => parse_data_chunk(input),
        b"FILE" => parse_file_chunk(input),
        unk => {
            panic!("Unknown chunk: {:?} {}", unk, input.previous_token_end());
        }
    }
}
