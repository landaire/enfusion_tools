use std::collections::VecDeque;
use std::ffi::OsStr;
use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;
use clap::Parser;
use enfusion_pak::Chunk;
use enfusion_pak::FileEntry;
use enfusion_pak::FileEntryMeta;
use enfusion_pak::PakFile;
use enfusion_pak::async_pak_vfs::AsyncPrime;
use enfusion_pak::pak_vfs::Prime;
use humansize::BINARY;
use humansize::format_size;
use memmap2::Mmap;

/// Simple program to greet a person
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, short)]
    long: bool,
    #[arg(long, short)]
    merged: bool,
    file: PathBuf,
}

pub fn add_num(integers: &mut Vec<u32>) {
    integers.push(0);
}

#[derive(Debug)]
#[allow(unused)]
struct WrappedPakFile {
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

fn parse_pak_files<P: AsRef<Path>>(files: &[P], args: &Args) -> color_eyre::Result<()> {
    let mut parsed_files = Vec::new();

    for file_path in files {
        let file_path: &Path = file_path.as_ref();
        let file = std::fs::File::open(file_path)?;
        let mmap = unsafe { memmap2::Mmap::map(&file)? };

        match PakFile::parse(&mmap) {
            Ok(pak_file) => {
                parsed_files.push(WrappedPakFile {
                    path: file_path.to_path_buf(),
                    source: mmap,
                    pak_file,
                });
            }
            Err(e) => {
                eprintln!("Error parsing {:?}: {:?}", file_path, e);
            }
        }
    }

    if args.merged {
        let mut merged_file = if let Some(i) =
            parsed_files.iter().rev().position(|file| file.pak_file.file_chunk().is_some())
        {
            parsed_files.remove(i)
        } else {
            println!("No data files contained a FILE chunk");
            return Ok(());
        };

        let Chunk::File { fs: merged_fs } = merged_file.pak_file.file_chunk_mut().expect("??")
        else {
            unreachable!()
        };

        for other in parsed_files {
            let Some(Chunk::File { fs: other_fs }) = other.pak_file.file_chunk() else {
                continue;
            };

            merged_fs.merge(other_fs.clone());
        }

        print_pak_file_chunk_details(merged_fs, args);
    } else {
        for (idx, pak) in parsed_files.iter().enumerate() {
            println!(
                "File: {}",
                files
                    .get(idx)
                    .expect("failed to get file path?")
                    .as_ref()
                    .to_str()
                    .expect("failed to convert pak file path to str")
            );

            print_pak_file(&pak.pak_file, args)?;
        }
    }

    Ok(())
}

fn print_pak_file_chunk_details(fs: &FileEntry, args: &Args) {
    let mut fs_queue = VecDeque::new();
    fs_queue.push_front((PathBuf::from("Root"), fs));

    while let Some((parent_path, next)) = fs_queue.pop_front() {
        let this_path = parent_path.join(next.name());
        let meta = next.meta();
        match meta {
            FileEntryMeta::Folder { children } => {
                if children.is_empty() {
                    println!("\t{}", this_path.to_str().expect("failed to convert path to str"));
                }

                children.iter().for_each(|child| fs_queue.push_back((this_path.clone(), child)));
            }
            FileEntryMeta::File {
                offset,
                compressed_len,
                decompressed_len,
                unk,
                unk2,
                compressed,
                compression_level,
                timestamp,
            } => {
                println!("\t{}", this_path.to_str().expect("failed to convert path to str"));

                if args.long {
                    println!("\t\tOffset: {:#X}", *offset);
                    println!(
                        "\t\tCompressed Size: {} ({} bytes)",
                        format_size(*compressed_len, BINARY),
                        *compressed_len
                    );
                    println!(
                        "\t\tDecompressed Size: {} ({} bytes)",
                        format_size(*decompressed_len, BINARY),
                        *decompressed_len
                    );
                    println!("\t\tUnknown #1: {:#X}", *unk);
                    println!("\t\tUnknown #2: {:#X}", *unk2);
                    println!(
                        "\t\tCompression Flags: {:#X}",
                        ((*compressed as u16) << 8) | (*compression_level as u16)
                    );
                    println!(
                        "\t\tTimestamp: {:?} ({})",
                        meta.parsed_timestamp().expect("file has invalid timestamp"),
                        *timestamp
                    )
                }
            }
        }
    }
}

fn print_pak_file(pak_file: &PakFile, args: &Args) -> color_eyre::Result<()> {
    for chunk in pak_file.chunks() {
        println!("Chunk {:?}", chunk.kind());
        match chunk {
            Chunk::Form { file_size, pak_file_type } => {
                println!("\tSize: {} ({} bytes)", format_size(*file_size, BINARY), *file_size);
                println!("\tVersion: {:?}", *pak_file_type);
            }
            Chunk::Head { version, unknown_data } => {
                println!("\tVersion: {:#X}", *version);
                println!("\tUnknown Data Len: {} bytes", (unknown_data.end - unknown_data.start));
            }
            Chunk::Data { data } => {
                println!("\tSize: {} ({} bytes)", format_size(data.len(), BINARY), data.len());
            }
            Chunk::File { fs } => {
                print_pak_file_chunk_details(fs, args);
            }
            Chunk::Unknown(_) => todo!(),
        }
        println!();
    }

    Ok(())
}

fn main() -> color_eyre::Result<()> {
    let args = Args::parse();

    if !args.file.exists() {
        println!("File does not exist");
        return Ok(());
    }

    let mut pak_files = Vec::new();
    if args.file.is_dir() {
        println!("it's a dir!");
        for entry in std::fs::read_dir(&args.file)? {
            let entry = entry?;
            let path = entry.path();
            if let Some("pak") = path.extension().and_then(OsStr::to_str) {
                pak_files.push(path);
            }
        }
    } else {
        pak_files.push(args.file.clone());
    }

    parse_pak_files(pak_files.as_ref(), &args)
}
