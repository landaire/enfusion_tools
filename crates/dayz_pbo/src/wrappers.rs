use std::collections::HashMap;
use std::fmt::Debug;
use std::ops::Range;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use log::debug;
use vfs::VfsError;
use vfs::VfsMetadata;
use vfs::error::VfsErrorKind;
use winnow::stream::Offset;
use winnow::stream::Stream as _;

use crate::ParserStateMachine;
use crate::PboFile;
use crate::PboParser;
use crate::Stream;
use crate::async_pbo_vfs::AsyncReadAt;
use crate::error::PboError;
use crate::pbo_vfs::VfsEntry;
use crate::pbo_vfs::build_entries;

/// Shared buffer wrapper for cached file data.
#[repr(transparent)]
#[derive(Clone, Debug)]
struct BufferWrapper(Arc<Vec<u8>>);

impl AsRef<[u8]> for BufferWrapper {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// An async wrapper around a parsed PBO file that reads data on-demand and
/// caches results with a bounded memory budget.
///
/// This mirrors `enfusion_pak::wrappers::async_reader::CachingAsyncPakFileWrapper`.
pub struct CachingAsyncPboFileWrapper<T> {
    handle: T,
    buffer: Mutex<HashMap<Range<usize>, BufferWrapper>>,
    pbo_file: PboFile,
}

impl<T: Debug> Debug for CachingAsyncPboFileWrapper<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachingAsyncPboFileWrapper")
            .field("handle", &self.handle)
            .field("pbo_file", &self.pbo_file)
            .finish()
    }
}

impl<T> CachingAsyncPboFileWrapper<T>
where
    T: AsyncReadAt + Clone + Send + Sync + 'static,
{
    /// Read (or return cached) data for the given byte range.
    async fn prime_file(&self, file_range: Range<usize>) -> Result<BufferWrapper, VfsError> {
        // Check cache first.
        {
            let buffers = self.buffer.lock().unwrap();
            if let Some(entry) = buffers.get(&file_range) {
                return Ok(entry.clone());
            }
        }

        // Cache miss — read from the underlying handle.
        let data = self.handle.read_at(file_range.clone()).await?;
        let vec: Vec<u8> = data.as_ref().to_vec();
        let wrapper = BufferWrapper(Arc::new(vec));

        let mut buffers = self.buffer.lock().unwrap();

        // Evict entries if we exceed the memory budget.
        let mut buffers_and_mem_usage =
            buffers.iter().map(|(k, v)| (k.clone(), v.0.len())).collect::<Vec<_>>();
        let mut mem_usage: usize = buffers_and_mem_usage.iter().map(|(_, m)| m).sum();

        // Don't consume more than 20 MiB
        const MEM_LIMIT: usize = 1024 * 1024 * 20;
        if mem_usage > MEM_LIMIT {
            // Evict largest entries first (same strategy as PAK).
            buffers_and_mem_usage.sort_by_key(|(_, v)| *v);
            for (k, v) in buffers_and_mem_usage {
                buffers.remove(&k);
                mem_usage -= v;
                if mem_usage < MEM_LIMIT {
                    break;
                }
            }
        }

        buffers.entry(file_range).insert_entry(wrapper.clone());

        Ok(wrapper)
    }
}

/// A virtual filesystem backed by a PBO archive that reads file data
/// on-demand via [`AsyncReadAt`], suitable for WASM where loading the
/// entire archive into memory would cause OOM.
///
/// Cloning is cheap (Arc refcount bump).
#[derive(Debug, Clone)]
pub struct AsyncPboVfs<T> {
    inner: Arc<CachingAsyncPboFileWrapper<T>>,
    entries: Arc<HashMap<String, VfsEntry>>,
}

impl<T> AsyncPboVfs<T>
where
    T: AsyncReadAt + Clone + Send + Sync + Debug + 'static,
{
    fn lookup(&self, path: &str) -> vfs::VfsResult<&VfsEntry> {
        let key = path.strip_prefix('/').unwrap_or(path);
        self.entries.get(key).ok_or_else(|| VfsError::from(VfsErrorKind::FileNotFound))
    }
}

impl<T> vfs::FileSystem for AsyncPboVfs<T>
where
    T: AsyncReadAt + Clone + Send + Sync + Debug + 'static,
{
    fn read_dir(&self, path: &str) -> vfs::VfsResult<Box<dyn Iterator<Item = String> + Send>> {
        let entry = self.lookup(path)?;
        match entry {
            VfsEntry::Directory { children } => Ok(Box::new(children.clone().into_iter())),
            VfsEntry::File { .. } => Err(VfsError::from(VfsErrorKind::NotSupported)),
        }
    }

    fn create_dir(&self, _path: &str) -> vfs::VfsResult<()> {
        Err(VfsErrorKind::NotSupported.into())
    }

    fn open_file(&self, _path: &str) -> vfs::VfsResult<Box<dyn vfs::SeekAndRead + Send>> {
        // Sync file reads are not supported in the async VFS wrapper.
        // The UI only uses the sync VFS path for directory listing / metadata.
        Err(VfsErrorKind::NotSupported.into())
    }

    fn create_file(&self, _path: &str) -> vfs::VfsResult<Box<dyn vfs::SeekAndWrite + Send>> {
        Err(VfsErrorKind::NotSupported.into())
    }

    fn append_file(&self, _path: &str) -> vfs::VfsResult<Box<dyn vfs::SeekAndWrite + Send>> {
        Err(VfsErrorKind::NotSupported.into())
    }

    fn metadata(&self, path: &str) -> vfs::VfsResult<VfsMetadata> {
        let entry = self.lookup(path)?;
        match entry {
            VfsEntry::Directory { .. } => Ok(VfsMetadata {
                file_type: vfs::VfsFileType::Directory,
                len: 0,
                created: None,
                modified: None,
                accessed: None,
            }),
            VfsEntry::File { entry_index } => {
                let header = &self.inner.pbo_file.entries[*entry_index];
                let len = if header.original_size > 0 {
                    header.original_size as u64
                } else {
                    header.data_size as u64
                };
                Ok(VfsMetadata {
                    file_type: vfs::VfsFileType::File,
                    len,
                    created: None,
                    modified: None,
                    accessed: None,
                })
            }
        }
    }

    fn exists(&self, path: &str) -> vfs::VfsResult<bool> {
        Ok(self.lookup(path).is_ok())
    }

    fn remove_file(&self, _path: &str) -> vfs::VfsResult<()> {
        Err(VfsErrorKind::NotSupported.into())
    }

    fn remove_dir(&self, _path: &str) -> vfs::VfsResult<()> {
        Err(VfsErrorKind::NotSupported.into())
    }

    fn set_creation_time(&self, _path: &str, _time: std::time::SystemTime) -> vfs::VfsResult<()> {
        Err(VfsErrorKind::NotSupported.into())
    }

    fn set_modification_time(
        &self,
        _path: &str,
        _time: std::time::SystemTime,
    ) -> vfs::VfsResult<()> {
        Err(VfsErrorKind::NotSupported.into())
    }

    fn set_access_time(&self, _path: &str, _time: std::time::SystemTime) -> vfs::VfsResult<()> {
        Err(VfsErrorKind::NotSupported.into())
    }

    fn copy_file(&self, _src: &str, _dest: &str) -> vfs::VfsResult<()> {
        Err(VfsErrorKind::NotSupported.into())
    }

    fn move_file(&self, _src: &str, _dest: &str) -> vfs::VfsResult<()> {
        Err(VfsErrorKind::NotSupported.into())
    }

    fn move_dir(&self, _src: &str, _dest: &str) -> vfs::VfsResult<()> {
        Err(VfsErrorKind::NotSupported.into())
    }
}

#[cfg(feature = "async_vfs")]
mod async_impl {
    use super::*;
    use async_std::io::Cursor as AsyncCursor;
    use async_std::io::Write;
    use async_std::stream;
    use async_std::stream::Stream;
    use vfs::VfsResult;
    use vfs::async_vfs::AsyncFileSystem;
    use vfs::async_vfs::SeekAndRead;

    #[async_trait]
    impl<T> AsyncFileSystem for AsyncPboVfs<T>
    where
        T: AsyncReadAt + Clone + Send + Sync + Debug + 'static,
    {
        async fn read_dir(
            &self,
            path: &str,
        ) -> VfsResult<Box<dyn Unpin + Stream<Item = String> + Send>> {
            let entry = self.lookup(path)?;
            match entry {
                VfsEntry::Directory { children } => {
                    Ok(Box::new(stream::from_iter(children.clone().into_iter())))
                }
                VfsEntry::File { .. } => Err(VfsError::from(VfsErrorKind::NotSupported)),
            }
        }

        async fn create_dir(&self, _path: &str) -> VfsResult<()> {
            Err(VfsErrorKind::NotSupported.into())
        }

        async fn open_file(&self, path: &str) -> VfsResult<Box<dyn SeekAndRead + Send + Unpin>> {
            let entry = self.lookup(path)?;
            match entry {
                VfsEntry::File { entry_index } => {
                    let range = self.inner.pbo_file.entry_data_range_by_index(*entry_index);
                    let data = self.inner.prime_file(range).await?;
                    Ok(Box::new(AsyncCursor::new(data.0.as_ref().to_vec())))
                }
                VfsEntry::Directory { .. } => Err(VfsError::from(VfsErrorKind::NotSupported)),
            }
        }

        async fn create_file(&self, _path: &str) -> VfsResult<Box<dyn Write + Send + Unpin>> {
            Err(VfsErrorKind::NotSupported.into())
        }

        async fn append_file(&self, _path: &str) -> VfsResult<Box<dyn Write + Send + Unpin>> {
            Err(VfsErrorKind::NotSupported.into())
        }

        async fn metadata(&self, path: &str) -> VfsResult<VfsMetadata> {
            <Self as vfs::FileSystem>::metadata(self, path)
        }

        async fn exists(&self, path: &str) -> VfsResult<bool> {
            <Self as vfs::FileSystem>::exists(self, path)
        }

        async fn remove_file(&self, _path: &str) -> VfsResult<()> {
            Err(VfsErrorKind::NotSupported.into())
        }

        async fn remove_dir(&self, _path: &str) -> VfsResult<()> {
            Err(VfsErrorKind::NotSupported.into())
        }

        async fn set_creation_time(
            &self,
            _path: &str,
            _time: std::time::SystemTime,
        ) -> VfsResult<()> {
            Err(VfsErrorKind::NotSupported.into())
        }

        async fn set_modification_time(
            &self,
            _path: &str,
            _time: std::time::SystemTime,
        ) -> VfsResult<()> {
            Err(VfsErrorKind::NotSupported.into())
        }

        async fn set_access_time(
            &self,
            _path: &str,
            _time: std::time::SystemTime,
        ) -> VfsResult<()> {
            Err(VfsErrorKind::NotSupported.into())
        }

        async fn copy_file(&self, _src: &str, _dest: &str) -> VfsResult<()> {
            Err(VfsErrorKind::NotSupported.into())
        }

        async fn move_file(&self, _src: &str, _dest: &str) -> VfsResult<()> {
            Err(VfsErrorKind::NotSupported.into())
        }

        async fn move_dir(&self, _src: &str, _dest: &str) -> VfsResult<()> {
            Err(VfsErrorKind::NotSupported.into())
        }
    }
}

// ---------------------------------------------------------------------------
// Incremental header parsing
// ---------------------------------------------------------------------------

/// Parse a PBO file's headers incrementally by reading 64k chunks via
/// [`AsyncReadAt`], then return an [`AsyncPboVfs`] that reads file data
/// on-demand.
///
/// This avoids loading the entire PBO into memory — only the (small) header
/// section is read during parsing. The (potentially huge) data section is
/// skipped over and only read when individual files are opened.
pub async fn parse_pbo_file<T>(handle: T) -> Result<AsyncPboVfs<T>, PboError>
where
    T: AsyncReadAt + Clone + Send + Sync + Debug + 'static,
{
    let mut parser = PboParser::new();

    const CHUNK_SIZE: usize = 1024 * 64;
    let mut offset: usize = 0;

    loop {
        let read_range = offset..(offset + CHUNK_SIZE);
        let read_handle = handle.clone();
        let data = read_handle
            .read_at(read_range)
            .await
            .map_err(|e| PboError::IoError(std::io::Error::other(e.to_string())))?;

        let mut input = Stream::new(data.as_ref());
        let start = input.checkpoint();

        match parser.parse(&mut input) {
            Ok(ParserStateMachine::Done(pbo_file)) => {
                debug!("PBO header parsing complete");

                let prefix = pbo_file
                    .extensions
                    .get("prefix")
                    .map(|p| p.replace('\\', "/"))
                    .unwrap_or_default();
                let entries = build_entries(&pbo_file, &prefix);

                return Ok(AsyncPboVfs {
                    inner: Arc::new(CachingAsyncPboFileWrapper {
                        handle,
                        buffer: Default::default(),
                        pbo_file,
                    }),
                    entries: Arc::new(entries),
                });
            }
            Ok(ParserStateMachine::Skip { count, parser: next_parser }) => {
                let consumed = input.checkpoint().offset_from(&start);
                debug!(
                    "Skipping {count:#X} bytes (offset advancing from {:#X} to {:#X})",
                    offset + consumed,
                    offset + consumed + count
                );
                offset += consumed + count;
                parser = next_parser;
            }
            Ok(ParserStateMachine::Continue(next_parser)) => {
                let consumed = input.checkpoint().offset_from(&start);
                offset += consumed;
                parser = next_parser;
            }
            Ok(ParserStateMachine::Loop(_)) => {
                unreachable!("Loop state should not be returned from parse()");
            }
            Err(e) => {
                panic!("error reading PBO file: {e:?}");
            }
        }
    }
}
