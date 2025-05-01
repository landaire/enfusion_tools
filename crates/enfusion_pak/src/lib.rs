use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::{self, Rc};
use std::{panic::PanicHookInfo, path::Path};

use error::PakError;
use jiff::civil::DateTime;
use kinded::{Kind, Kinded};
use variantly::Variantly;
use winnow::binary::{be_u32, le_u16, le_u32, u8};
use winnow::combinator::alt;
use winnow::error::ContextError;
use winnow::token::take;
use winnow::{Parser, Result as WResult};

mod error;

#[derive(Debug)]
struct FileEntry<'input> {
    name: Cow<'input, str>,
    meta: FileEntryMeta<'input>,
}

impl<'input> FileEntry<'input> {
    pub fn name(&self) -> &str {
        self.name.as_ref()
    }

    pub fn kind(&self) -> FileEntryKind {
        self.meta.kind()
    }

    pub fn timestamp(&self) -> Option<jiff::civil::DateTime> {
        match self.meta {
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

#[derive(Debug, Kinded, Variantly)]
#[kinded(kind = FileEntryKind)]
enum FileEntryMeta<'input> {
    Folder {
        children: HashMap<String, FileEntry<'input>>,
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

impl<'input> FileEntryMeta<'input> {
    /// Adds a child to this file entry. No-op if this is a folder
    pub fn push_child(&mut self, child: FileEntry<'input>) {
        if let FileEntryMeta::Folder { children } = self {
            children.insert(child.name().to_owned(), child);
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
pub struct PakFile {}

#[derive(Debug)]
enum PakType {
    PAC1,
}

#[derive(Debug, Kinded, Variantly)]
enum Chunk<'input> {
    Pac1(u32),
    Form {
        file_size: u32,
        pak_file_type: PakType,
    },
    Head {
        version: u32,
        header_data: &'input [u8],
    },
    Data {
        data: &'input [u8],
    },
    File {
        fs: FileEntry<'input>,
    },
    Unknown(u32),
}

impl PakFile {
    pub fn parse(path: &PathBuf) -> Result<PakFile, PakError> {
        let file = std::fs::File::open(path)?;
        let file = unsafe { memmap2::Mmap::map(&file)? };

        parse_pak(&file[..])
    }
}

fn parse_pak(mut input: &[u8]) -> Result<PakFile, PakError> {
    let result = PakFile {};

    loop {
        match parse_chunk(&mut input) {
            Ok(chunk) => {
                if let Chunk::Data { data } = chunk {
                    println!("Data chunk with len: {:#02X}", data.len());
                } else {
                    println!("{:#?}", chunk.kind());
                }
            }
            Err(e) => {
                println!("{:?}", e);
                break;
            }
        }
    }

    Ok(result)
}

fn parse_form_chunk<'input>(input: &mut &'input [u8]) -> WResult<Chunk<'input>> {
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
    Ok(Chunk::Form {
        file_size,
        pak_file_type,
    })
}

fn parse_head_chunk<'input>(input: &mut &'input [u8]) -> WResult<Chunk<'input>> {
    let header_len = be_u32(input)?;
    assert_eq!(header_len, 0x1c);

    let mut header_data = take(header_len).parse_next(input)?;
    let version = le_u32(&mut header_data)?;

    let chunk = Chunk::Head {
        version,
        header_data,
    };

    Ok(chunk)
}

fn parse_data_chunk<'input>(input: &mut &'input [u8]) -> WResult<Chunk<'input>> {
    let data_len = be_u32(input)?;

    let data = take(data_len).parse_next(input)?;

    let chunk = Chunk::Data { data };

    Ok(chunk)
}

fn parse_file_entry<'input>(input: &mut &'input [u8]) -> WResult<(FileEntry<'input>, usize)> {
    let entry_kind: FileEntryKind = u8(input)?.try_into().expect("???");
    let name_len = u8(input)?;
    let name_bytes = take(name_len).parse_next(input)?;
    // TODO: use proper error type
    let name = str::from_utf8(name_bytes).expect("invalid utf8 filename");

    let mut is_root = false;
    let (meta, children) = match entry_kind {
        FileEntryKind::Folder => {
            let children_count = le_u32(input)?;
            // Special case for root directory
            if name.is_empty() {
                is_root = true;
            }

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

            if compressed_len != decompressed_len {
                println!(
                    "compression_type: {unknown}, filename: {name}, unk: {unknown}, unk2: {unk2}, compressed: {compressed}, level: {compression_level}"
                );
            }
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

    let name = if is_root {
        Cow::Owned("Root".to_string())
    } else {
        Cow::Borrowed(name)
    };

    Ok((FileEntry { name, meta }, children))
}

fn parse_file_chunk<'input>(input: &mut &'input [u8]) -> WResult<Chunk<'input>> {
    let chunk_len = be_u32(input)?;

    dbg!(chunk_len);

    let mut chunk_data = take(chunk_len).parse_next(input)?;
    dbg!(chunk_data.len());

    struct Directory<'input> {
        is_root: bool,
        children_remaining: usize,
        entry: FileEntry<'input>,
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

    Ok(chunk)
}

fn parse_chunk<'input>(input: &mut &'input [u8]) -> WResult<Chunk<'input>> {
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
            panic!("Unknown chunk: {:?}", unk);
        }
    }
}
