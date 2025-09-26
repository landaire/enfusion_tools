use std::ops::Range;

use crate::error::PakError;
use jiff::civil::DateTime;
use kinded::Kinded;
use log::debug;
use variantly::Variantly;
pub use winnow::LocatingSlice;
use winnow::ModalResult as WResult;
use winnow::Parser;
pub use winnow::Partial;
use winnow::binary::be_u32;
use winnow::binary::le_u16;
use winnow::binary::le_u32;
use winnow::binary::u8;
use winnow::combinator::alt;
use winnow::error::ErrMode;
use winnow::error::StrContext;
use winnow::stream::Offset;
use winnow::stream::Stream as _;
use winnow::token::take;

/// Represents some type of a file or directory
#[derive(Debug, Clone)]
pub struct FileEntry {
    name: String,
    meta: FileEntryMeta,
}

#[cfg(feature = "arc")]
pub type RcFileEntry = std::sync::Arc<FileEntry>;

#[cfg(not(feature = "arc"))]
pub type RcFileEntry = std::rc::Rc<FileEntry>;

impl FileEntry {
    /// Entry's name
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    /// What kind of entry this is
    pub fn kind(&self) -> FileEntryKind {
        self.meta.kind()
    }

    /// Entry metadata. For a directory this will contain its children,
    /// and for a file this will contain file metadata.
    pub fn meta(&self) -> &FileEntryMeta {
        &self.meta
    }

    /// Merges `other` into this node.
    pub fn merge(&mut self, other: Self) {
        let FileEntryMeta::Folder { children: self_children } = &mut self.meta else {
            panic!("merge should only be called on directories");
        };

        let FileEntryMeta::Folder { children: other_children } = other.meta else {
            panic!("merge should only be called on directories");
        };

        for other_child in other_children {
            if let Some(self_child) =
                self_children.iter_mut().find(|self_child| self_child.name == other_child.name)
            {
                if other_child.kind() == FileEntryKind::File {
                    debug!("{:#?}, {:#?}", &self_child, &other_child);
                }
                assert_eq!(other_child.kind(), self_child.kind());
                assert_ne!(
                    other_child.kind(),
                    FileEntryKind::File,
                    "File was duplicated across PAK files"
                );
                RcFileEntry::get_mut(self_child)
                    .expect("couldn't get self_child as mut")
                    .merge(RcFileEntry::try_unwrap(other_child).expect("couldn't unwrap child"));
            } else {
                self_children.push(other_child);
            }
        }
    }

    /// Merges refcounted children from `other` into this node.
    pub fn merge_ref(&mut self, other: RcFileEntry) {
        let FileEntryMeta::Folder { children: self_children } = &mut self.meta else {
            panic!("merge should only be called on directories");
        };

        let FileEntryMeta::Folder { children: other_children } = &other.meta else {
            panic!("merge should only be called on directories");
        };

        for other_child in other_children {
            if let Some(self_child) =
                self_children.iter_mut().find(|self_child| self_child.name == other_child.name)
            {
                if other_child.kind() == FileEntryKind::File {
                    debug!("{:#?}, {:#?}", &self_child, &other_child);
                }
                assert_eq!(other_child.kind(), self_child.kind());
                assert_ne!(
                    other_child.kind(),
                    FileEntryKind::File,
                    "File was duplicated across PAK files"
                );
                RcFileEntry::get_mut(self_child)
                    .expect("couldn't get self_child as mut")
                    .merge_ref(RcFileEntry::clone(other_child));
            } else {
                self_children.push(RcFileEntry::clone(other_child));
            }
        }
    }
}

/// An entry's metadata containing either its children or file metadata
#[derive(Debug, Clone, Kinded, Variantly)]
#[kinded(kind = FileEntryKind)]
#[non_exhaustive]
pub enum FileEntryMeta {
    Folder {
        children: Vec<RcFileEntry>,
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
            children.push(RcFileEntry::new(child));
        }
    }

    /// Returns this file's timestamp. For directories there is no timestamp information
    /// and this will return `None`. For Files, this returns the date/time at which the file
    /// was modified(?). Note: there is no time zone information recorded.
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
                panic!("unknown file entry kind: {value:#X}");
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
    /// Returns an immutable slice of the [`Chunk`]s contained in this `PakFile`.
    pub fn chunks(&self) -> &[Chunk] {
        &self.chunks
    }

    /// Returns a mutable `Vec` for this `PakFile`'s chunks.
    pub fn chunks_mut(&mut self) -> &mut Vec<Chunk> {
        &mut self.chunks
    }

    /// Finds and returns an immutable reference to the [`Chunk::File`] chunk contained in this `PakFile`.
    /// May return `None` if no such chunk exists.
    pub fn file_chunk(&self) -> Option<&Chunk> {
        self.chunks.iter().find(|chunk| chunk.is_file())
    }

    /// Finds and returns a mutable reference to the [`Chunk::File`] chunk contained in this `PakFile`.
    /// May return `None` if no such chunk exists.
    pub fn file_chunk_mut(&mut self) -> Option<&mut Chunk> {
        self.chunks.iter_mut().find(|chunk| chunk.is_file())
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum PakType {
    PAC1,
}

#[derive(Debug, Kinded, Variantly)]
#[non_exhaustive]
pub enum Chunk {
    Form {
        file_size: u32,
        pak_file_type: PakType,
    },
    Head {
        version: u32,
        unknown_data: Range<usize>,
    },
    /// Contains the
    Data {
        data: Range<usize>,
    },
    File {
        fs: RcFileEntry,
    },
    Unknown(u32),
}

impl PakFile {
    pub fn parse(data: &[u8]) -> Result<PakFile, PakError> {
        let mut parser = PakParser::new();

        let mut curr_data = data;
        loop {
            let mut input = Stream::new(curr_data);
            let start = input.checkpoint();
            // For mmap the parser should never raise an error or require state transitions
            match parser.parse(&mut input) {
                Ok(ParserStateMachine::Done(pak_file)) => {
                    return Ok(pak_file);
                }
                Ok(ParserStateMachine::Skip { from, count, parser: next_parser }) => {
                    let parsed = input.checkpoint().offset_from(&start);
                    curr_data = &curr_data[parsed..];

                    debug!(
                        "Skipping {count:#X} bytes from {from:#X} (offset is now: {:#X})",
                        next_parser.bytes_parsed()
                    );
                    curr_data = &curr_data[count..];
                    parser = next_parser;
                }
                Ok(state) => {
                    panic!("Unexpected state: {:?}", state.kind());
                }
                Err(winnow::error::ErrMode::Cut(e)) => {
                    return Err(PakError::ParserError(e));
                }
                Err(e) => {
                    panic!("Unknown error occurred: {e:?}");
                }
            }
        }
    }
}

pub struct PakParser {
    state: PakParserState,
    chunks: Vec<Chunk>,
    pak_len: Option<usize>,
    bytes_parsed: usize,
}

pub type Stream<'i> = Partial<&'i [u8]>;

impl PakParser {
    pub fn new() -> Self {
        PakParser {
            state: PakParserState::ParsingChunk,
            chunks: Vec::with_capacity(4),
            pak_len: None,
            bytes_parsed: 0,
        }
    }

    pub fn bytes_parsed(&self) -> usize {
        self.bytes_parsed
    }

    fn next_state(&mut self, bytes_consumed: usize, override_state: Option<PakParserState>) {
        debug!("Consumed {:#X} bytes from offset {:#X}", bytes_consumed, self.bytes_parsed);
        self.bytes_parsed += bytes_consumed;

        if let Some(override_state) = override_state {
            self.state = override_state
        } else {
            let old_state = std::mem::replace(&mut self.state, PakParserState::ParsingChunk);
            self.state = match old_state {
                PakParserState::ParsingChunk => PakParserState::ParsingChunk,
                PakParserState::ParsingFileChunk {
                    parsed_root,
                    mut parents,
                    bytes_processed,
                    chunk_len,
                } => {
                    if bytes_processed + bytes_consumed == chunk_len {
                        assert_eq!(parents.len(), 1);
                        self.chunks.push(Chunk::File {
                            fs: RcFileEntry::new(parents.pop().unwrap().entry),
                        });
                        PakParserState::Done
                    } else {
                        PakParserState::ParsingFileChunk {
                            parsed_root,
                            parents,
                            bytes_processed: bytes_processed + bytes_consumed,
                            chunk_len,
                        }
                    }
                }
                PakParserState::Done => PakParserState::Done,
            };
        }

        // If we've reached the end of the file, we're done. This check must come last
        // to ensure all of the above state transitions can handle anything they need
        // to do.
        if let Some(pak_len) = self.pak_len
            && self.bytes_parsed == pak_len
        {
            self.state = PakParserState::Done;
        }
    }

    pub fn parse_impl(mut self, input: &mut Stream) -> WResult<ParserStateMachine> {
        let start = input.checkpoint();
        debug!("Beginning read from {:#X}", self.bytes_parsed);

        match &self.state {
            PakParserState::ParsingChunk => {
                debug!("Reading a chunk");
                let res = parse_chunk(input);
                let parsed = match res {
                    Ok(parsed) => parsed,
                    Err(ErrMode::Incomplete(_)) => {
                        input.reset(&start);
                        return Ok(ParserStateMachine::Continue(self));
                    }
                    Err(e) => {
                        return Err(e);
                    }
                };
                debug!("Read complete! Result: {:#?}", parsed.kind());

                // Parse a single chunk
                let (skip, chunk, state) = match parsed {
                    Parsed::Chunk(chunk) => (0, Some(chunk), None),
                    Parsed::ChunkAndSkip(skip, chunk) => (skip, Some(chunk), None),
                    Parsed::FileChunkHeader { chunk_len } => {
                        debug!(
                            "We have {:#X} bytes to read starting at {:#X}",
                            chunk_len, self.bytes_parsed
                        );
                        let override_state = PakParserState::ParsingFileChunk {
                            parsed_root: false,
                            parents: Vec::with_capacity(4),
                            chunk_len,
                            bytes_processed: 0,
                        };

                        (0, None, Some(override_state))
                    }
                };

                let bytes_consumed = input.checkpoint().offset_from(&start);
                self.next_state(input.checkpoint().offset_from(&start) + skip, state);

                let skip_from = self.bytes_parsed - skip;

                if let Some(chunk) = chunk
                    && let Chunk::Form { file_size, .. } = &chunk
                {
                    // TODO: we shouldn't read the PAC1 data here
                    self.pak_len = Some((*file_size as usize) + (bytes_consumed - 4));
                    self.chunks.push(chunk);
                }

                if skip > 0 {
                    return Ok(ParserStateMachine::Skip {
                        from: skip_from,
                        count: skip,
                        parser: self,
                    });
                }
            }
            PakParserState::ParsingFileChunk { .. } => {
                debug!("Reading the file chunk");
                // Continue parsing file chunk
                match self.parse_file_entry(input) {
                    Ok(_) => {
                        // do nothing
                    }
                    Err(ErrMode::Incomplete(_)) => {
                        debug!("incomplete while reading file entry");
                        input.reset(&start);
                        return Ok(ParserStateMachine::Continue(self));
                    }
                    Err(e) => {
                        debug!("hard error while reading file entry");
                        return Err(e);
                    }
                }
                self.next_state(input.checkpoint().offset_from(&start), None);
            }
            PakParserState::Done => {
                debug!("Done");
                unreachable!("Loop should exist before PakParserState::Done is ever reached")
            }
        }

        Ok(ParserStateMachine::Loop(self))
    }

    pub fn parse(mut self, input: &mut Stream) -> WResult<ParserStateMachine> {
        // Parse as much data as we can. We either complete, or have to forward a state
        // machine quest up the stack.
        while !matches!(self.state, PakParserState::Done) {
            let next_state = self.parse_impl(input)?;

            if let ParserStateMachine::Loop(this) = next_state {
                self = this;
                continue;
            }

            return Ok(next_state);
        }

        Ok(ParserStateMachine::Done(self.complete()))
    }

    fn parse_file_entry(&mut self, input: &mut Stream) -> WResult<()> {
        let PakParserState::ParsingFileChunk { parsed_root, parents, .. } = &mut self.state else {
            panic!("Ended up in parse_file_entry in the wrong state")
        };

        let (entry, children) = parse_file_entry(input)?;

        match entry.meta.kind() {
            FileEntryKind::Folder => {
                parents.push(Directory {
                    is_root: !*parsed_root,
                    children_remaining: children,
                    entry,
                });

                *parsed_root = true;
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
            let parent =
                parents.last_mut().expect("expected a folder to have a parent, but there is none");

            parent.children_remaining = parent
                .children_remaining
                .checked_sub(1)
                .expect("encountered more children than expected for a folder");

            parent.entry.meta.push_child(dir.entry);
        }

        Ok(())
    }

    pub fn complete(self) -> PakFile {
        PakFile { chunks: self.chunks }
    }
}

impl Default for PakParser {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
struct Directory {
    is_root: bool,
    children_remaining: usize,
    entry: FileEntry,
}

#[derive(Debug)]
enum PakParserState {
    ParsingChunk,
    ParsingFileChunk {
        parsed_root: bool,
        parents: Vec<Directory>,
        bytes_processed: usize,
        chunk_len: usize,
    },
    Done,
}

impl PakParserState {}

#[derive(Kinded)]
enum Parsed {
    Chunk(Chunk),
    ChunkAndSkip(usize, Chunk),
    FileChunkHeader { chunk_len: usize },
}

#[derive(Kinded)]
pub enum ParserStateMachine {
    Loop(PakParser),
    Continue(PakParser),
    Skip { from: usize, count: usize, parser: PakParser },
    Done(PakFile),
}

fn parse_file_entry(input: &mut Stream) -> WResult<(FileEntry, usize)> {
    let entry_kind: FileEntryKind = u8(input)?.try_into().expect("???");
    let name_len = u8(input)?;
    let name = take(name_len).parse_next(input)?;
    let name =
        String::from_utf8(name.to_vec()).expect("name does not contain valid UTF8 characters");

    let (meta, children) = match entry_kind {
        FileEntryKind::Folder => {
            let children_count = le_u32(input)?;
            (FileEntryMeta::Folder { children: Default::default() }, children_count as usize)
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

fn parse_form_chunk(input: &mut Stream) -> WResult<Parsed> {
    let file_size = be_u32(input)?;
    let pak_type_bytes: [u8; 4] = take(4usize)
        .parse_next(input)?
        .try_into()
        .expect("winnow should have returned a 4-byte buffer");
    let pak_file_type = match &pak_type_bytes {
        b"PAC1" => PakType::PAC1,
        unk => {
            panic!("unknown pak type: {unk:?}");
        }
    };

    Ok(Parsed::Chunk(Chunk::Form { file_size, pak_file_type }))
}

fn parse_head_chunk(input: &mut Stream) -> WResult<Parsed> {
    let head_start = input.checkpoint();
    let header_len = be_u32.parse_next(input)? as usize;
    assert_eq!(header_len, 0x1c);

    let mut skip_bytes = 0;

    let header_data_start = input.checkpoint();
    let version = le_u32.parse_next(input)?;
    let unknown_data_start = input.checkpoint();
    skip_bytes += unknown_data_start.offset_from(&header_data_start);

    let unknown_data_offset = unknown_data_start.offset_from(&head_start);

    let chunk = Chunk::Head {
        version,
        unknown_data: unknown_data_offset..(unknown_data_offset + skip_bytes),
    };

    Ok(Parsed::ChunkAndSkip(header_len - skip_bytes, chunk))
}

fn parse_data_chunk(input: &mut Stream) -> WResult<Parsed> {
    let data_chunk_start = input.checkpoint();
    let data_len = be_u32.parse_next(input)? as usize;

    let data_data_start = input.checkpoint();
    let data_data_offset = data_data_start.offset_from(&data_chunk_start);
    //take(data_len).void().parse_next(input)?;

    let chunk = Chunk::Data { data: data_data_offset..(data_data_offset + data_len) };

    Ok(Parsed::ChunkAndSkip(data_len, chunk))
}

fn parse_file_chunk(input: &mut Stream) -> WResult<Parsed> {
    let chunk_len = be_u32(input)? as usize;
    Ok(Parsed::FileChunkHeader { chunk_len })
}

fn parse_chunk(input: &mut Stream) -> WResult<Parsed> {
    alt((
        (b"FORM", parse_form_chunk)
            .context(StrContext::Label("chunk"))
            .context(StrContext::Expected(winnow::error::StrContextValue::Description("FORM"))),
        (b"HEAD", parse_head_chunk)
            .context(StrContext::Label("chunk"))
            .context(StrContext::Expected(winnow::error::StrContextValue::Description("HEAD"))),
        (b"DATA", parse_data_chunk)
            .context(StrContext::Label("chunk"))
            .context(StrContext::Expected(winnow::error::StrContextValue::Description("DATA"))),
        (b"FILE", parse_file_chunk)
            .context(StrContext::Label("chunk"))
            .context(StrContext::Expected(winnow::error::StrContextValue::Description("FILE"))),
    ))
    .parse_next(input)
    .map(|(_, parsed)| parsed)
}
