use std::collections::BTreeMap;
use std::ops::Range;

use crate::error::PboError;
use log::debug;
use winnow::ModalResult as WResult;
use winnow::Parser;
use winnow::binary::le_u32;
use winnow::error::ErrMode;
use winnow::error::StrContext;
use winnow::stream::Offset;
use winnow::stream::Stream as _;
use winnow::token::take;
use winnow::token::take_while;

/// Packing method for a header entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackingMethod {
    /// Uncompressed file data.
    Uncompressed,
    /// LZH-compressed file data (packing method "Cprs" / 0x43707273).
    Compressed,
    /// Version/product header extension entry (packing method "Vers" / 0x56657273).
    /// The entry itself carries no file data; it is followed by key-value
    /// extension pairs terminated by an empty string.
    Vers,
}

impl PackingMethod {
    fn from_u32(v: u32) -> Self {
        match v {
            0x0000_0000 => PackingMethod::Uncompressed,
            0x4370_7273 => PackingMethod::Compressed,
            0x5665_7273 => PackingMethod::Vers,
            other => {
                panic!("unknown packing method: {other:#010X}");
            }
        }
    }
}

/// A single header entry describing one file inside the PBO archive.
#[derive(Debug, Clone)]
pub struct HeaderEntry {
    /// File path relative to the PBO prefix (backslash-separated).
    pub filename: String,
    /// How the file data is stored.
    pub packing_method: PackingMethod,
    /// Original (decompressed) size. Non-zero only when `packing_method` is
    /// [`PackingMethod::Compressed`].
    pub original_size: u32,
    /// Reserved field – always 0 in every PBO observed so far.
    pub reserved: u32,
    /// Unix timestamp of the file (seconds since epoch).
    pub timestamp: u32,
    /// Size of the file's data blob inside the data section.
    pub data_size: u32,
}

/// Parsed representation of a PBO archive.
#[derive(Debug, Clone)]
pub struct PboFile {
    /// Header extension key-value pairs (e.g. "product", "prefix", "version").
    /// Populated from the Vers header entry. Empty if no Vers entry was present.
    pub extensions: BTreeMap<String, String>,
    /// All file entries from the header (excludes the Vers and terminator entries).
    pub entries: Vec<HeaderEntry>,
    /// Byte range of the data section within the original input buffer.
    pub data_range: Range<usize>,
    /// The 20-byte SHA-1 checksum stored at the end of the file, if present.
    pub checksum: Option<[u8; 20]>,
}

impl PboFile {
    /// Parse a complete PBO archive from a byte slice.
    pub fn parse(data: &[u8]) -> Result<PboFile, PboError> {
        let mut parser = PboParser::new();
        let mut curr_data = data;

        loop {
            let mut input = Stream::new(curr_data);
            let start = input.checkpoint();

            match parser.parse(&mut input) {
                Ok(ParserStateMachine::Done(pbo)) => return Ok(pbo),
                Ok(ParserStateMachine::Skip { count, parser: next_parser }) => {
                    let parsed = input.checkpoint().offset_from(&start);
                    curr_data = &curr_data[parsed..];

                    debug!(
                        "Skipping {count:#X} bytes (offset is now: {:#X})",
                        next_parser.bytes_parsed()
                    );
                    curr_data = &curr_data[count..];
                    parser = next_parser;
                }
                Ok(ParserStateMachine::Loop(_)) => {
                    panic!("unexpected Loop state returned from parse()");
                }
                Ok(ParserStateMachine::Continue(_)) => {
                    panic!("unexpected Continue – parse() was given the full buffer");
                }
                Err(ErrMode::Cut(e)) => return Err(PboError::ParserError(e)),
                Err(e) => {
                    panic!("unexpected parser error: {e:?}");
                }
            }
        }
    }
}

pub type Stream<'i> = winnow::Partial<&'i [u8]>;

pub struct PboParser {
    state: PboParserState,
    extensions: BTreeMap<String, String>,
    entries: Vec<HeaderEntry>,
    bytes_parsed: usize,
    total_len: Option<usize>,
}

enum PboParserState {
    /// Parsing header entries one by one.
    ParsingHeaders,
    /// Expecting header extension key-value pairs after a Vers entry.
    ParsingExtensions,
    /// All headers parsed; we know how large the data section is and need to
    /// skip over it.
    SkippingData {
        data_start: usize,
        data_len: usize,
    },
    /// Expecting the trailing checksum.
    ParsingChecksum {
        data_start: usize,
        data_len: usize,
    },
    Done,
}

pub enum ParserStateMachine {
    Loop(PboParser),
    Continue(PboParser),
    Skip { count: usize, parser: PboParser },
    Done(PboFile),
}

impl PboParser {
    pub fn new() -> Self {
        PboParser {
            state: PboParserState::ParsingHeaders,
            extensions: BTreeMap::new(),
            entries: Vec::new(),
            bytes_parsed: 0,
            total_len: None,
        }
    }

    pub fn bytes_parsed(&self) -> usize {
        self.bytes_parsed
    }

    pub fn with_total_len(mut self, len: usize) -> Self {
        self.total_len = Some(len);
        self
    }

    pub fn parse(mut self, input: &mut Stream) -> WResult<ParserStateMachine> {
        while !matches!(self.state, PboParserState::Done) {
            let next = self.parse_step(input)?;
            match next {
                ParserStateMachine::Loop(this) => {
                    self = this;
                    continue;
                }
                other => return Ok(other),
            }
        }
        Ok(ParserStateMachine::Done(self.complete()))
    }

    fn parse_step(mut self, input: &mut Stream) -> WResult<ParserStateMachine> {
        let start = input.checkpoint();

        match &self.state {
            PboParserState::ParsingHeaders => {
                let entry = match parse_header_entry(input) {
                    Ok(e) => e,
                    Err(ErrMode::Incomplete(_)) => {
                        input.reset(&start);
                        return Ok(ParserStateMachine::Continue(self));
                    }
                    Err(e) => return Err(e),
                };

                let bytes_consumed = input.checkpoint().offset_from(&start);
                self.bytes_parsed += bytes_consumed;

                if entry.filename.is_empty()
                    && entry.packing_method == PackingMethod::Vers
                    && entry.data_size == 0
                    && entry.timestamp == 0
                {
                    debug!("Found Vers header entry, switching to extension parsing");
                    self.state = PboParserState::ParsingExtensions;
                } else if entry.filename.is_empty() {
                    debug!("Found terminator entry at offset {:#X}", self.bytes_parsed);
                    let data_len: usize = self.entries.iter().map(|e| e.data_size as usize).sum();
                    let data_start = self.bytes_parsed;
                    self.state = PboParserState::SkippingData { data_start, data_len };
                } else {
                    debug!(
                        "Header entry: {:?} (size={}, ts={:#X})",
                        entry.filename, entry.data_size, entry.timestamp
                    );
                    self.entries.push(entry);
                }
            }

            PboParserState::ParsingExtensions => {
                let (key, value) = match parse_extension_pair(input) {
                    Ok(kv) => kv,
                    Err(ErrMode::Incomplete(_)) => {
                        input.reset(&start);
                        return Ok(ParserStateMachine::Continue(self));
                    }
                    Err(e) => return Err(e),
                };

                let bytes_consumed = input.checkpoint().offset_from(&start);
                self.bytes_parsed += bytes_consumed;

                if key.is_empty() {
                    debug!("End of header extensions");
                    self.state = PboParserState::ParsingHeaders;
                } else {
                    debug!("Extension: {key}={value}");
                    self.extensions.insert(key, value);
                }
            }

            PboParserState::SkippingData { data_start, data_len } => {
                let data_start = *data_start;
                let data_len = *data_len;
                self.state = PboParserState::ParsingChecksum { data_start, data_len };
                return Ok(ParserStateMachine::Skip { count: data_len, parser: self });
            }

            PboParserState::ParsingChecksum { data_start, data_len } => {
                let data_start = *data_start;
                let data_len = *data_len;

                let checksum = match parse_checksum(input) {
                    Ok(c) => c,
                    Err(ErrMode::Incomplete(_)) => {
                        input.reset(&start);
                        return Ok(ParserStateMachine::Continue(self));
                    }
                    Err(e) => return Err(e),
                };

                let bytes_consumed = input.checkpoint().offset_from(&start);
                self.bytes_parsed += bytes_consumed;

                self.state = PboParserState::Done;

                return Ok(ParserStateMachine::Done(PboFile {
                    extensions: self.extensions,
                    entries: self.entries,
                    data_range: data_start..(data_start + data_len),
                    checksum,
                }));
            }

            PboParserState::Done => unreachable!(),
        }

        Ok(ParserStateMachine::Loop(self))
    }

    fn complete(self) -> PboFile {
        let data_len: usize = self.entries.iter().map(|e| e.data_size as usize).sum();
        PboFile {
            extensions: self.extensions,
            entries: self.entries,
            data_range: self.bytes_parsed..(self.bytes_parsed + data_len),
            checksum: None,
        }
    }
}

impl Default for PboParser {
    fn default() -> Self {
        Self::new()
    }
}

impl PboFile {
    /// Returns the byte range within the original input for a given entry's data.
    pub fn entry_data_range(&self, entry: &HeaderEntry) -> Range<usize> {
        let mut offset = self.data_range.start;
        for e in &self.entries {
            if std::ptr::eq(e, entry) {
                return offset..(offset + e.data_size as usize);
            }
            offset += e.data_size as usize;
        }
        panic!("entry not found in this PboFile");
    }

    /// Returns the byte range within the original input for the entry at `index`.
    pub fn entry_data_range_by_index(&self, index: usize) -> Range<usize> {
        let mut offset = self.data_range.start;
        for (i, e) in self.entries.iter().enumerate() {
            if i == index {
                return offset..(offset + e.data_size as usize);
            }
            offset += e.data_size as usize;
        }
        panic!("entry index {index} out of range (have {} entries)", self.entries.len());
    }

    /// Convenience: get the data for a file entry from the original buffer.
    pub fn entry_data<'a>(&self, data: &'a [u8], index: usize) -> &'a [u8] {
        let range = self.entry_data_range_by_index(index);
        &data[range]
    }
}

// ---------------------------------------------------------------------------
// Winnow parsers
// ---------------------------------------------------------------------------

/// Read a null-terminated string.
fn parse_nul_string(input: &mut Stream) -> WResult<String> {
    let bytes = take_while(0.., |b| b != 0u8).parse_next(input)?;
    let _ = winnow::binary::u8(input)?; // consume the NUL
    let s = String::from_utf8(bytes.to_vec()).expect("PBO filename/extension is not valid UTF-8");
    Ok(s)
}

fn parse_header_entry(input: &mut Stream) -> WResult<HeaderEntry> {
    let filename = parse_nul_string.context(StrContext::Label("filename")).parse_next(input)?;

    let packing_method_raw =
        le_u32.context(StrContext::Label("packing_method")).parse_next(input)?;
    let packing_method = PackingMethod::from_u32(packing_method_raw);

    let original_size = le_u32.context(StrContext::Label("original_size")).parse_next(input)?;
    let reserved = le_u32.context(StrContext::Label("reserved")).parse_next(input)?;
    let timestamp = le_u32.context(StrContext::Label("timestamp")).parse_next(input)?;
    let data_size = le_u32.context(StrContext::Label("data_size")).parse_next(input)?;

    Ok(HeaderEntry { filename, packing_method, original_size, reserved, timestamp, data_size })
}

/// Parse a single key-value extension pair (two NUL-terminated strings).
/// When the key is empty the extensions section is over.
fn parse_extension_pair(input: &mut Stream) -> WResult<(String, String)> {
    let key = parse_nul_string.context(StrContext::Label("extension key")).parse_next(input)?;
    if key.is_empty() {
        return Ok((String::new(), String::new()));
    }
    let value = parse_nul_string.context(StrContext::Label("extension value")).parse_next(input)?;
    Ok((key, value))
}

/// Parse the optional trailing checksum: a NUL byte followed by 20 bytes of
/// SHA-1 hash. Returns `None` if we're already at EOF.
fn parse_checksum(input: &mut Stream) -> WResult<Option<[u8; 20]>> {
    // Check if there is any data left
    if input.is_empty() {
        return Ok(None);
    }

    let _nul =
        winnow::binary::u8.context(StrContext::Label("checksum separator")).parse_next(input)?;

    let hash_bytes: &[u8] =
        take(20usize).context(StrContext::Label("sha1 checksum")).parse_next(input)?;

    let mut hash = [0u8; 20];
    hash.copy_from_slice(hash_bytes);
    Ok(Some(hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal PBO in memory for unit testing.
    fn build_test_pbo() -> Vec<u8> {
        let mut buf = Vec::new();

        // -- Vers header entry --
        buf.push(0x00); // empty filename NUL
        buf.extend_from_slice(&0x5665_7273u32.to_le_bytes()); // packing method = Vers
        buf.extend_from_slice(&0u32.to_le_bytes()); // original_size
        buf.extend_from_slice(&0u32.to_le_bytes()); // reserved
        buf.extend_from_slice(&0u32.to_le_bytes()); // timestamp
        buf.extend_from_slice(&0u32.to_le_bytes()); // data_size

        // -- Extensions --
        buf.extend_from_slice(b"product\0test\0");
        buf.extend_from_slice(b"prefix\0MyMod\0");
        buf.push(0x00); // extensions terminator

        // -- File entry: hello.txt --
        buf.extend_from_slice(b"hello.txt\0");
        buf.extend_from_slice(&0u32.to_le_bytes()); // uncompressed
        buf.extend_from_slice(&0u32.to_le_bytes()); // original_size
        buf.extend_from_slice(&0u32.to_le_bytes()); // reserved
        buf.extend_from_slice(&1000u32.to_le_bytes()); // timestamp
        buf.extend_from_slice(&5u32.to_le_bytes()); // data_size = 5

        // -- File entry: dir\sub.bin --
        buf.extend_from_slice(b"dir\\sub.bin\0");
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&2000u32.to_le_bytes());
        buf.extend_from_slice(&3u32.to_le_bytes()); // data_size = 3

        // -- Terminator entry --
        buf.push(0x00); // empty filename
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());

        // -- Data section --
        buf.extend_from_slice(b"hello"); // 5 bytes for hello.txt
        buf.extend_from_slice(&[0xAA, 0xBB, 0xCC]); // 3 bytes for dir\sub.bin

        // -- Checksum --
        buf.push(0x00); // separator
        buf.extend_from_slice(&[0u8; 20]); // dummy SHA-1

        buf
    }

    #[test]
    fn parse_synthetic_pbo() {
        let data = build_test_pbo();
        let pbo = PboFile::parse(&data).expect("failed to parse synthetic PBO");

        assert_eq!(pbo.extensions.get("product").unwrap(), "test");
        assert_eq!(pbo.extensions.get("prefix").unwrap(), "MyMod");
        assert_eq!(pbo.entries.len(), 2);

        assert_eq!(pbo.entries[0].filename, "hello.txt");
        assert_eq!(pbo.entries[0].data_size, 5);
        assert_eq!(pbo.entries[0].timestamp, 1000);

        assert_eq!(pbo.entries[1].filename, "dir\\sub.bin");
        assert_eq!(pbo.entries[1].data_size, 3);

        // Verify data extraction
        let hello_data = pbo.entry_data(&data, 0);
        assert_eq!(hello_data, b"hello");

        let sub_data = pbo.entry_data(&data, 1);
        assert_eq!(sub_data, &[0xAA, 0xBB, 0xCC]);

        // Verify checksum
        assert_eq!(pbo.checksum, Some([0u8; 20]));
    }

    #[test]
    fn parse_pbo_without_vers() {
        let mut buf = Vec::new();

        // File entry directly (no Vers)
        buf.extend_from_slice(b"test.txt\0");
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&500u32.to_le_bytes());
        buf.extend_from_slice(&4u32.to_le_bytes());

        // Terminator
        buf.push(0x00);
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());

        // Data
        buf.extend_from_slice(b"abcd");

        // No checksum

        let pbo = PboFile::parse(&buf).expect("failed to parse PBO without Vers");
        assert!(pbo.extensions.is_empty());
        assert_eq!(pbo.entries.len(), 1);
        assert_eq!(pbo.entries[0].filename, "test.txt");
        assert_eq!(pbo.entry_data(&buf, 0), b"abcd");
        assert_eq!(pbo.checksum, None);
    }

    #[test]
    fn data_ranges_are_correct() {
        let data = build_test_pbo();
        let pbo = PboFile::parse(&data).expect("failed to parse");

        let r0 = pbo.entry_data_range_by_index(0);
        let r1 = pbo.entry_data_range_by_index(1);

        // Ranges should be contiguous
        assert_eq!(r0.end, r1.start);
        assert_eq!(r0.len(), 5);
        assert_eq!(r1.len(), 3);
    }
}
