use std::collections::HashMap;
use std::fmt::Debug;
use std::io::Cursor;
use std::ops::Range;

use vfs::FileSystem;
use vfs::VfsError;
use vfs::VfsMetadata;
use vfs::error::VfsErrorKind;

use crate::Chunk;
use crate::FileEntryMeta;
use crate::PakFile;
use crate::RcFileEntry;

/// Trait which allows for requesting a file be read into memory.
pub trait Prime {
    /// Request the provided `file_range` be primed and returned.
    fn prime_file(&self, file_range: Range<usize>) -> impl AsRef<[u8]>;
}

/// Synchronous VFS implementation for reading a `.pak` file.
#[derive(Debug, Clone)]
pub struct PakVfs<T> {
    pub(crate) source: T,

    entry_cache: HashMap<String, RcFileEntry>,
}

impl<T> PakVfs<T>
where
    T: std::ops::Deref,
    T::Target: AsRef<PakFile>,
{
    /// Construct a new `PakVfs` from the provided `source` which can be represented as a `PakFile` reference.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use enfusion_pak::PakFile;
    /// use enfusion_pak::pak_vfs::PakVfs;
    /// use enfusion_pak::error::PakError;
    ///
    /// fn parse_pak_file(path: PathBuf) -> Result<PakFile, PakError> {
    ///     let file = std::fs::File::open(&path)?;
    ///     let mmap = unsafe { memmap2::Mmap::map(&file)? };
    ///
    ///     PakFile::parse(&mmap)
    /// }
    ///
    /// let parsed_file = parse_pak_file()?;
    /// let vfs = PakVfs::new(Arc::new(parsed_file));
    /// ```
    pub fn new(source: T) -> Self {
        // Generate the cache
        let mut entry_cache = HashMap::new();
        let pak: &PakFile = source.as_ref();
        let file_chunk = pak.file_chunk().unwrap();
        let Chunk::File { fs } = file_chunk else { panic!("file chunk is not a file?") };

        let mut queue = vec![("".to_string(), RcFileEntry::clone(fs))];
        while let Some((path, current)) = queue.pop() {
            let this_path = if path == "/" {
                format!("{}{}", path, current.name())
            } else {
                format!("{}/{}", path, current.name())
            };
            let key = this_path.clone();
            entry_cache.insert(key, RcFileEntry::clone(&current));

            match current.meta() {
                FileEntryMeta::Folder { children } => {
                    for child in children {
                        queue.push((this_path.clone(), RcFileEntry::clone(child)));
                    }
                }
                FileEntryMeta::File { .. } => {
                    // files don't need any action
                }
            }
        }
        Self { source, entry_cache }
    }
}

impl<T> PakVfs<T>
where
    T: std::ops::Deref,
    T::Target: AsRef<PakFile>,
{
    /// Look up a file entry by its path.
    pub fn entry_at(&self, path: &str) -> vfs::VfsResult<&RcFileEntry> {
        let lookup_key = if path.is_empty() { "/" } else { path };

        self.entry_cache.get(lookup_key).ok_or_else(|| VfsError::from(VfsErrorKind::FileNotFound))

        // if let Some(cached) = self.entry_cache.get(lookup_key) {
        //     return Ok(cached);
        // }

        // let pak: &PakFile = self.source.as_ref();
        // let file_chunk = pak.file_chunk().unwrap();
        // let Chunk::File { fs } = file_chunk else { panic!("file chunk is not a file?") };
        // let mut current: &RcFileEntry = fs;

        // let path_parts = if path.starts_with("/") {
        //     path.split('/').skip(1)
        // } else {
        //     #[allow(clippy::iter_skip_zero)]
        //     path.split('/').skip(0)
        // };

        // for part in path_parts {
        //     if part.is_empty() {
        //         continue;
        //     }

        //     let FileEntryMeta::Folder { children } = current.meta() else {
        //         return Err(VfsError::from(VfsErrorKind::NotSupported));
        //     };

        //     if let Some(next) = children.iter().find(|child| child.name() == part) {
        //         current = next;
        //     } else {
        //         return Err(VfsError::from(VfsErrorKind::FileNotFound));
        //     }
        // }

        // // Add this to the cache
        // self.entry_cache.insert(lookup_key.to_string(), RcFileEntry::clone(&current));

        // Ok(current)
    }
}

#[cfg(feature = "arc")]
impl<T> FileSystem for PakVfs<T>
where
    T: std::ops::Deref + Sync + Send + Debug + 'static,
    T::Target: AsRef<PakFile> + Prime,
{
    fn read_dir(&self, path: &str) -> vfs::VfsResult<Box<dyn Iterator<Item = String> + Send>> {
        let entry = self.entry_at(path)?;

        match entry.meta() {
            FileEntryMeta::Folder { children } => Ok(Box::new(
                children
                    .iter()
                    .map(|child| child.name().to_string())
                    .collect::<Vec<_>>()
                    .into_iter(),
            )),
            FileEntryMeta::File { .. } => Err(VfsError::from(VfsErrorKind::NotSupported)),
        }
    }

    fn create_dir(&self, _path: &str) -> vfs::VfsResult<()> {
        todo!()
    }

    fn open_file(&self, path: &str) -> vfs::VfsResult<Box<dyn vfs::SeekAndRead + Send>> {
        let entry = self.entry_at(path)?;
        let FileEntryMeta::File { offset, compressed_len, decompressed_len, compressed, .. } =
            entry.meta()
        else {
            return Err(VfsError::from(VfsErrorKind::NotSupported));
        };

        let mut data = Vec::with_capacity(*decompressed_len as usize);
        let data_start = *offset as usize;
        let data_end = data_start + *compressed_len as usize;

        let primed_file = self.source.prime_file(data_start..data_end);
        let source_slice: &[u8] = primed_file.as_ref();
        let mut source_range = source_slice;
        if *compressed != 0 {
            let mut decoder = flate2::read::ZlibDecoder::new(source_range);
            std::io::copy(&mut decoder, &mut data).map_err(|err| {
                println!("error occurred during decompression: {err:#?}");
                println!("offset: {:#X?}", *offset);
                VfsError::from(VfsErrorKind::IoError(err))
            })?;

            Ok(Box::new(Cursor::new(data)))
        } else {
            let _ = std::io::copy(&mut source_range, &mut data);

            Ok(Box::new(Cursor::new(data)))
        }
    }

    fn create_file(&self, _path: &str) -> vfs::VfsResult<Box<dyn vfs::SeekAndWrite + Send>> {
        todo!()
    }

    fn append_file(&self, _path: &str) -> vfs::VfsResult<Box<dyn vfs::SeekAndWrite + Send>> {
        todo!()
    }

    fn metadata(&self, path: &str) -> vfs::VfsResult<vfs::VfsMetadata> {
        let entry = self.entry_at(path)?;

        let pak_meta = entry.meta();
        let meta = match pak_meta {
            FileEntryMeta::Folder { children: _ } => VfsMetadata {
                file_type: vfs::VfsFileType::Directory,
                len: 0,
                created: None,
                modified: None,
                accessed: None,
            },
            FileEntryMeta::File { decompressed_len, .. } => {
                // let converted_timestamp = pak_meta.parsed_timestamp().map(|ts| {
                //     SystemTime::fr
                // })
                VfsMetadata {
                    file_type: vfs::VfsFileType::File,
                    len: *decompressed_len as u64,
                    created: None,
                    modified: None,
                    accessed: None,
                }
            }
        };

        Ok(meta)
    }

    fn exists(&self, path: &str) -> vfs::VfsResult<bool> {
        if self.entry_at(path).is_ok() {
            return Ok(true);
        }

        Ok(false)
    }

    fn remove_file(&self, _path: &str) -> vfs::VfsResult<()> {
        todo!()
    }

    fn remove_dir(&self, _path: &str) -> vfs::VfsResult<()> {
        todo!()
    }

    fn set_creation_time(&self, _path: &str, _time: std::time::SystemTime) -> vfs::VfsResult<()> {
        Err(vfs::VfsError::from(vfs::error::VfsErrorKind::NotSupported))
    }

    fn set_modification_time(
        &self,
        _path: &str,
        _time: std::time::SystemTime,
    ) -> vfs::VfsResult<()> {
        Err(vfs::VfsError::from(vfs::error::VfsErrorKind::NotSupported))
    }

    fn set_access_time(&self, _path: &str, _time: std::time::SystemTime) -> vfs::VfsResult<()> {
        Err(vfs::VfsError::from(vfs::error::VfsErrorKind::NotSupported))
    }

    fn copy_file(&self, _src: &str, _dest: &str) -> vfs::VfsResult<()> {
        Err(vfs::error::VfsErrorKind::NotSupported.into())
    }

    fn move_file(&self, _src: &str, _dest: &str) -> vfs::VfsResult<()> {
        Err(vfs::error::VfsErrorKind::NotSupported.into())
    }

    fn move_dir(&self, _src: &str, _dest: &str) -> vfs::VfsResult<()> {
        Err(vfs::error::VfsErrorKind::NotSupported.into())
    }
}
