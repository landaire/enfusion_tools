use std::fmt::Write;
use std::io::Cursor;

use byteorder::LittleEndian;
use byteorder::ReadBytesExt;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RapError {
    #[error("invalid magic: expected \\0raP header")]
    InvalidMagic,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unknown entry type {0} at offset {1:#x}")]
    UnknownEntryType(u8, u64),
    #[error("unknown value subtype {0} at offset {1:#x}")]
    UnknownValueSubtype(u8, u64),
    #[error("invalid UTF-8 string at offset {0:#x}")]
    InvalidString(u64),
}

/// Parsed representation of a rapified config file.
#[derive(Debug, Clone)]
pub struct RapFile {
    pub root: RapClass,
}

#[derive(Debug, Clone)]
pub struct RapClass {
    pub parent: String,
    pub entries: Vec<RapEntry>,
}

#[derive(Debug, Clone)]
pub enum RapEntry {
    Class { name: String, body: RapClass },
    Property { name: String, value: RapValue },
    Array { name: String, elements: Vec<RapValue> },
    Delete(String),
}

#[derive(Debug, Clone)]
pub enum RapValue {
    String(String),
    Float(f32),
    Int(i32),
}

/// Read a null-terminated ASCII string from a cursor.
fn read_asciiz(cursor: &mut Cursor<&[u8]>) -> Result<String, RapError> {
    let pos = cursor.position();
    let mut buf = Vec::new();
    loop {
        let b = cursor.read_u8()?;
        if b == 0 {
            break;
        }
        buf.push(b);
    }
    String::from_utf8(buf).map_err(|_| RapError::InvalidString(pos))
}

/// Read a compressed integer (1 or 2 bytes, high bit signals continuation).
fn read_compressed_int(cursor: &mut Cursor<&[u8]>) -> Result<u32, RapError> {
    let b0 = cursor.read_u8()? as u32;
    if b0 & 0x80 == 0 {
        Ok(b0)
    } else {
        let b1 = cursor.read_u8()? as u32;
        Ok((b0 & 0x7f) | (b1 << 7))
    }
}

/// Read a typed value (string/float/int) based on a subtype tag.
fn read_typed_value(cursor: &mut Cursor<&[u8]>, subtype: u8) -> Result<RapValue, RapError> {
    match subtype {
        0 => Ok(RapValue::String(read_asciiz(cursor)?)),
        1 => Ok(RapValue::Float(cursor.read_f32::<LittleEndian>()?)),
        2 => Ok(RapValue::Int(cursor.read_i32::<LittleEndian>()?)),
        _ => Err(RapError::UnknownValueSubtype(subtype, cursor.position())),
    }
}

fn read_class_body(full_data: &[u8], offset: u64) -> Result<RapClass, RapError> {
    let mut cursor = Cursor::new(full_data);
    cursor.set_position(offset);

    let parent = read_asciiz(&mut cursor)?;
    let n_entries = read_compressed_int(&mut cursor)?;
    let mut entries = Vec::with_capacity(n_entries as usize);

    for _ in 0..n_entries {
        let entry_pos = cursor.position();
        let entry_type = cursor.read_u8()?;

        match entry_type {
            0 => {
                // Subclass: name + absolute offset to class body
                let name = read_asciiz(&mut cursor)?;
                let body_offset = cursor.read_u32::<LittleEndian>()? as u64;
                let body = read_class_body(full_data, body_offset)?;
                entries.push(RapEntry::Class { name, body });
            }
            1 => {
                // Property: subtype + name + value
                let subtype = cursor.read_u8()?;
                let name = read_asciiz(&mut cursor)?;
                let value = read_typed_value(&mut cursor, subtype)?;
                entries.push(RapEntry::Property { name, value });
            }
            2 => {
                // Array: name + compressed count + typed elements
                let name = read_asciiz(&mut cursor)?;
                let count = read_compressed_int(&mut cursor)?;
                let mut elements = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    let elem_type = cursor.read_u8()?;
                    elements.push(read_typed_value(&mut cursor, elem_type)?);
                }
                entries.push(RapEntry::Array { name, elements });
            }
            3 => {
                // Delete marker
                let name = read_asciiz(&mut cursor)?;
                entries.push(RapEntry::Delete(name));
            }
            _ => return Err(RapError::UnknownEntryType(entry_type, entry_pos)),
        }
    }

    Ok(RapClass { parent, entries })
}

impl RapFile {
    /// Parse a rapified config.bin from raw bytes.
    pub fn parse(data: &[u8]) -> Result<Self, RapError> {
        if data.len() < 16 || &data[0..4] != b"\x00raP" {
            return Err(RapError::InvalidMagic);
        }

        // Header: [0..4] magic, [4..8] reserved, [8..12] reserved, [12..16] enum_offset
        // Root class body starts at offset 16
        let root = read_class_body(data, 16)?;
        Ok(RapFile { root })
    }
}

/// Format a parsed rapified config as human-readable class definitions.
pub fn decompile(rap: &RapFile) -> String {
    let mut out = String::new();
    write_class_entries(&mut out, &rap.root.entries, 0);
    out
}

fn write_class_entries(out: &mut String, entries: &[RapEntry], indent: usize) {
    let prefix: String = "\t".repeat(indent);
    for entry in entries {
        match entry {
            RapEntry::Class { name, body } => {
                if body.parent.is_empty() {
                    let _ = writeln!(out, "{prefix}class {name}");
                } else {
                    let _ = writeln!(out, "{prefix}class {name} : {}", body.parent);
                }
                let _ = writeln!(out, "{prefix}{{");
                write_class_entries(out, &body.entries, indent + 1);
                let _ = writeln!(out, "{prefix}}};");
            }
            RapEntry::Property { name, value } => {
                let _ = writeln!(out, "{prefix}{name} = {};", format_value(value));
            }
            RapEntry::Array { name, elements } => {
                let vals: Vec<String> = elements.iter().map(format_value).collect();
                let _ = writeln!(out, "{prefix}{name}[] = {{{}}};", vals.join(", "));
            }
            RapEntry::Delete(name) => {
                let _ = writeln!(out, "{prefix}delete {name};");
            }
        }
    }
}

fn format_value(v: &RapValue) -> String {
    match v {
        RapValue::String(s) => format!("\"{s}\""),
        RapValue::Float(f) => format!("{f}"),
        RapValue::Int(i) => format!("{i}"),
    }
}

/// Returns `true` if the given data starts with the rapified config magic bytes.
pub fn is_rapified(data: &[u8]) -> bool {
    data.len() >= 4 && &data[0..4] == b"\x00raP"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_magic() {
        assert!(is_rapified(b"\x00raP\x00\x00\x00\x00"));
        assert!(!is_rapified(b"not a config"));
        assert!(!is_rapified(b"\x00ra"));
    }
}
