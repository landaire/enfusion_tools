use std::fmt::Debug;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::ops::Range;
use std::sync::Arc;

use fskit::Metadata;
use fskit::ReadOnlyVfs;
use fskit::VfsTree;
use fskit::VfsTreeBuilder;
use vfs::VfsMetadata;
use vfs::VfsResult;

use crate::PboFile;

/// File metadata stored in the VFS tree for each PBO entry.
#[derive(Debug, Clone)]
pub struct PboFileMeta {
    /// Index into `PboFile::entries`.
    pub entry_index: usize,
    /// Uncompressed file size.
    pub len: u64,
}

impl Metadata for PboFileMeta {
    fn len(&self) -> u64 {
        self.len
    }
}

/// Build a [`VfsTree`] from a parsed PBO header.
///
/// If `prefix` is non-empty (e.g. `"DZ/AI"`), all file paths are rooted
/// under that prefix so multiple PBOs can be overlaid into one coherent tree.
pub fn build_tree(pbo: &PboFile, prefix: &str) -> VfsTree<PboFileMeta> {
    let mut builder = VfsTreeBuilder::new();

    if !prefix.is_empty() {
        builder = builder.insert_dir(prefix, None);
    }

    for (idx, header) in pbo.entries.iter().enumerate() {
        let relative = header.filename.replace('\\', "/");
        let full_path = if prefix.is_empty() { relative } else { format!("{prefix}/{relative}") };

        let len = if header.original_size > 0 {
            header.original_size as u64
        } else {
            header.data_size as u64
        };

        builder = builder.insert(full_path, PboFileMeta { entry_index: idx, len });
    }

    builder.build()
}

// ---------------------------------------------------------------------------
// Zero-copy reader
// ---------------------------------------------------------------------------

/// Zero-copy reader over a subslice of an `Arc`-backed buffer.
/// Holds a cheap `Arc` clone instead of copying file data into a `Vec`.
struct ArcSliceReader<T> {
    source: Arc<T>,
    range: Range<usize>,
    pos: usize,
}

impl<T: AsRef<[u8]>> ArcSliceReader<T> {
    fn remaining(&self) -> &[u8] {
        let all: &[u8] = (*self.source).as_ref();
        let slice = &all[self.range.clone()];
        &slice[self.pos..]
    }
}

impl<T: AsRef<[u8]>> Read for ArcSliceReader<T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.remaining();
        let n = std::cmp::min(buf.len(), remaining.len());
        buf[..n].copy_from_slice(&remaining[..n]);
        self.pos += n;
        Ok(n)
    }
}

impl<T: AsRef<[u8]>> Seek for ArcSliceReader<T> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let len = self.range.len() as i64;
        let new_pos = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::End(n) => len + n,
            SeekFrom::Current(n) => self.pos as i64 + n,
        };
        if new_pos < 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before start"));
        }
        self.pos = new_pos as usize;
        Ok(self.pos as u64)
    }
}

// ---------------------------------------------------------------------------
// Opener
// ---------------------------------------------------------------------------

/// Opener that reads PBO file data via zero-copy `ArcSliceReader`.
#[derive(Debug, Clone)]
pub struct PboOpener<T> {
    source: Arc<T>,
    pbo: PboFile,
}

impl<T> PboOpener<T> {
    fn data_range(&self, meta: &PboFileMeta) -> Range<usize> {
        self.pbo.entry_data_range_by_index(meta.entry_index)
    }

    fn make_reader(&self, meta: &PboFileMeta) -> ArcSliceReader<T> {
        ArcSliceReader { source: Arc::clone(&self.source), range: self.data_range(meta), pos: 0 }
    }
}

impl<T> fskit::FileOpener<PboFileMeta> for PboOpener<T>
where
    T: AsRef<[u8]> + Debug + Send + Sync + 'static,
{
    fn open(&self, meta: &PboFileMeta) -> vfs::VfsResult<Box<dyn vfs::SeekAndRead + Send>> {
        Ok(Box::new(self.make_reader(meta)))
    }
}

#[cfg(feature = "async_vfs")]
const _: () = {
    /// Async adapter over `ArcSliceReader`. No real async I/O since data is
    /// already in memory.
    struct AsyncSliceReader<T>(ArcSliceReader<T>);
    impl<T> Unpin for AsyncSliceReader<T> {}

    impl<T: AsRef<[u8]>> async_std::io::Read for AsyncSliceReader<T> {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut [u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Read::read(&mut self.0, buf))
        }
    }

    impl<T: AsRef<[u8]>> async_std::io::Seek for AsyncSliceReader<T> {
        fn poll_seek(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            pos: SeekFrom,
        ) -> std::task::Poll<std::io::Result<u64>> {
            std::task::Poll::Ready(Seek::seek(&mut self.0, pos))
        }
    }

    impl<T> fskit::AsyncFileOpener<PboFileMeta> for PboOpener<T>
    where
        T: AsRef<[u8]> + Debug + Send + Sync + 'static,
    {
        fn open_async(
            &self,
            meta: &PboFileMeta,
        ) -> vfs::VfsResult<Box<dyn vfs::async_vfs::SeekAndRead + Send + Unpin>> {
            Ok(Box::new(AsyncSliceReader(self.make_reader(meta))))
        }
    }
};

// ---------------------------------------------------------------------------
// PboVfs
// ---------------------------------------------------------------------------

/// A virtual filesystem backed by a PBO archive.
///
/// `T` is the backing store (e.g. `memmap2::Mmap`, `Vec<u8>`) that provides
/// the raw PBO bytes.
///
/// Cloning is cheap (Arc refcount bump).
#[derive(Debug, Clone)]
pub struct PboVfs<T>(ReadOnlyVfs<PboFileMeta, PboOpener<T>>);

impl<T> PboVfs<T>
where
    T: AsRef<[u8]> + Debug + Send + Sync + 'static,
{
    pub fn new(source: T, pbo: PboFile) -> Self {
        let prefix = pbo.extensions.get("prefix").map(|p| p.replace('\\', "/")).unwrap_or_default();
        let tree = build_tree(&pbo, &prefix);
        let opener = PboOpener { source: Arc::new(source), pbo };
        Self(ReadOnlyVfs::new(tree, opener))
    }
}

impl<T> vfs::FileSystem for PboVfs<T>
where
    T: AsRef<[u8]> + Debug + Send + Sync + 'static,
{
    fn read_dir(&self, path: &str) -> VfsResult<Box<dyn Iterator<Item = String> + Send>> {
        self.0.read_dir(path)
    }
    fn create_dir(&self, path: &str) -> VfsResult<()> {
        self.0.create_dir(path)
    }
    fn open_file(&self, path: &str) -> VfsResult<Box<dyn vfs::SeekAndRead + Send>> {
        self.0.open_file(path)
    }
    fn create_file(&self, path: &str) -> VfsResult<Box<dyn vfs::SeekAndWrite + Send>> {
        self.0.create_file(path)
    }
    fn append_file(&self, path: &str) -> VfsResult<Box<dyn vfs::SeekAndWrite + Send>> {
        self.0.append_file(path)
    }
    fn metadata(&self, path: &str) -> VfsResult<VfsMetadata> {
        self.0.metadata(path)
    }
    fn exists(&self, path: &str) -> VfsResult<bool> {
        self.0.exists(path)
    }
    fn remove_file(&self, path: &str) -> VfsResult<()> {
        self.0.remove_file(path)
    }
    fn remove_dir(&self, path: &str) -> VfsResult<()> {
        self.0.remove_dir(path)
    }
}

#[cfg(feature = "async_vfs")]
const _: () = {
    use async_trait::async_trait;
    use vfs::async_vfs::AsyncFileSystem;

    #[async_trait]
    impl<T> AsyncFileSystem for PboVfs<T>
    where
        T: AsRef<[u8]> + Debug + Send + Sync + 'static,
    {
        async fn read_dir(
            &self,
            path: &str,
        ) -> VfsResult<Box<dyn Unpin + futures::Stream<Item = String> + Send>> {
            self.0.read_dir(path).await
        }
        async fn create_dir(&self, path: &str) -> VfsResult<()> {
            self.0.create_dir(path).await
        }
        async fn open_file(
            &self,
            path: &str,
        ) -> VfsResult<Box<dyn vfs::async_vfs::SeekAndRead + Send + Unpin>> {
            self.0.open_file(path).await
        }
        async fn create_file(
            &self,
            path: &str,
        ) -> VfsResult<Box<dyn async_std::io::Write + Send + Unpin>> {
            self.0.create_file(path).await
        }
        async fn append_file(
            &self,
            path: &str,
        ) -> VfsResult<Box<dyn async_std::io::Write + Send + Unpin>> {
            self.0.append_file(path).await
        }
        async fn metadata(&self, path: &str) -> VfsResult<VfsMetadata> {
            self.0.metadata(path).await
        }
        async fn exists(&self, path: &str) -> VfsResult<bool> {
            self.0.exists(path).await
        }
        async fn remove_file(&self, path: &str) -> VfsResult<()> {
            self.0.remove_file(path).await
        }
        async fn remove_dir(&self, path: &str) -> VfsResult<()> {
            self.0.remove_dir(path).await
        }
    }
};
