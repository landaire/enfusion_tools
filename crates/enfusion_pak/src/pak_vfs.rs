use std::fmt::Debug;
use std::io::Cursor;
use std::ops::Range;

use fskit::Metadata;
use fskit::VfsTree;
use fskit::VfsTreeBuilder;
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
    fn prime_file(&self, file_range: Range<usize>) -> Result<impl AsRef<[u8]>, VfsError>;
}

pub trait ReadAt {
    fn read_at(&self, file_range: std::ops::Range<usize>) -> Result<impl AsRef<[u8]>, VfsError>;
}

/// File metadata stored in the VFS tree for each PAK entry.
#[derive(Debug, Clone)]
pub struct PakFileMeta {
    pub offset: u32,
    pub compressed_len: u32,
    pub decompressed_len: u32,
    pub compressed: u8,
}

impl Metadata for PakFileMeta {
    fn len(&self) -> u64 {
        self.decompressed_len as u64
    }
}

/// Build a [`VfsTree`] from a parsed PAK file.
fn build_tree(pak: &PakFile) -> VfsTree<PakFileMeta> {
    let file_chunk = pak.file_chunk().unwrap();
    let Chunk::File { fs } = file_chunk else { panic!("file chunk is not a file?") };

    let mut builder = VfsTreeBuilder::new();

    let mut queue = vec![("".to_string(), RcFileEntry::clone(fs))];
    while let Some((path, current)) = queue.pop() {
        let this_path = if path == "/" {
            format!("{}{}", path, current.name())
        } else {
            format!("{}/{}", path, current.name())
        };

        match current.meta() {
            FileEntryMeta::Folder { children } => {
                builder = builder.insert_dir(&this_path, None);
                for child in children {
                    queue.push((this_path.clone(), RcFileEntry::clone(child)));
                }
            }
            FileEntryMeta::File {
                offset, compressed_len, decompressed_len, compressed, ..
            } => {
                builder = builder.insert(
                    &this_path,
                    PakFileMeta {
                        offset: *offset,
                        compressed_len: *compressed_len,
                        decompressed_len: *decompressed_len,
                        compressed: *compressed,
                    },
                );
            }
        }
    }

    builder.build()
}

/// Synchronous VFS implementation for reading a `.pak` file.
#[derive(Debug, Clone)]
pub struct PakVfs<T> {
    pub(crate) source: T,
    tree: VfsTree<PakFileMeta>,
}

impl<T> PakVfs<T>
where
    T: std::ops::Deref,
    T::Target: AsRef<PakFile>,
{
    /// Construct a new `PakVfs` from the provided `source`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::sync::Arc;
    /// use enfusion_pak::PakFile;
    /// use enfusion_pak::pak_vfs::PakVfs;
    /// use enfusion_pak::wrappers::sync_reader::CachingPakFileWrapper;
    ///
    /// let path = std::path::PathBuf::from("example.pak");
    /// let file = std::fs::File::open(&path).unwrap();
    /// let mmap = unsafe { memmap2::Mmap::map(&file).unwrap() };
    /// let parsed_file = PakFile::parse(&mmap).unwrap();
    /// let wrapper = CachingPakFileWrapper::new(path, file, parsed_file);
    /// let vfs = PakVfs::new(Arc::new(wrapper));
    /// ```
    pub fn new(source: T) -> Self {
        let pak: &PakFile = source.as_ref();
        let tree = build_tree(pak);
        Self { source, tree }
    }

    pub fn tree(&self) -> &VfsTree<PakFileMeta> {
        &self.tree
    }
}

fn open_pak_data<T>(
    source: &T,
    meta: &PakFileMeta,
) -> vfs::VfsResult<Box<dyn vfs::SeekAndRead + Send>>
where
    T: std::ops::Deref,
    T::Target: Prime,
{
    let mut data = Vec::with_capacity(meta.decompressed_len as usize);
    let data_start = meta.offset as usize;
    let data_end = data_start + meta.compressed_len as usize;

    let primed_file = source.prime_file(data_start..data_end)?;
    let source_slice: &[u8] = primed_file.as_ref();
    let mut source_range = source_slice;
    if meta.compressed != 0 {
        let mut decoder = flate2::read::ZlibDecoder::new(source_range);
        std::io::copy(&mut decoder, &mut data).map_err(|err| {
            println!("error occurred during decompression: {err:#?}");
            println!("offset: {:#X?}", meta.offset);
            VfsError::from(VfsErrorKind::IoError(err))
        })?;
    } else {
        let _ = std::io::copy(&mut source_range, &mut data);
    }

    Ok(Box::new(Cursor::new(data)))
}

#[cfg(feature = "arc")]
impl<T> vfs::FileSystem for PakVfs<T>
where
    T: std::ops::Deref + Sync + Send + Debug + 'static,
    T::Target: AsRef<PakFile> + Prime,
{
    fn read_dir(&self, path: &str) -> vfs::VfsResult<Box<dyn Iterator<Item = String> + Send>> {
        self.tree.vfs_read_dir(path)
    }

    fn open_file(&self, path: &str) -> vfs::VfsResult<Box<dyn vfs::SeekAndRead + Send>> {
        let entry = self.tree.vfs_lookup(path)?;
        let fskit::VfsEntry::File(meta) = entry else {
            return Err(VfsError::from(VfsErrorKind::Other("not a file".into())));
        };
        open_pak_data(&self.source, meta)
    }

    fn metadata(&self, path: &str) -> vfs::VfsResult<VfsMetadata> {
        self.tree.vfs_metadata(path)
    }

    fn exists(&self, path: &str) -> vfs::VfsResult<bool> {
        self.tree.vfs_exists(path)
    }

    fskit::read_only_fs_stubs!();
}
