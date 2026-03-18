use std::collections::HashMap;
use std::fmt::Debug;
use std::io::Cursor;
use std::sync::Arc;

use vfs::VfsError;
use vfs::VfsMetadata;
use vfs::error::VfsErrorKind;

use crate::PboFile;

/// A VFS entry in the synthetic directory tree.
#[derive(Debug, Clone)]
enum VfsEntry {
    Directory {
        children: Vec<String>,
    },
    File {
        /// Index into `PboFile::entries`.
        entry_index: usize,
    },
}

/// Shared inner data for a PBO VFS.
#[derive(Debug)]
struct PboVfsInner<T> {
    source: T,
    pbo: PboFile,
    entries: HashMap<String, VfsEntry>,
}

/// A virtual filesystem backed by a PBO archive.
///
/// `T` is the backing store (e.g. `memmap2::Mmap`, `Vec<u8>`) that provides
/// the raw PBO bytes.  The parsed [`PboFile`] carries the header/offset
/// information needed to locate individual file data.
///
/// Cloning is cheap (Arc refcount bump).
#[derive(Debug, Clone)]
pub struct PboVfs<T> {
    inner: Arc<PboVfsInner<T>>,
}

impl<T> PboVfs<T>
where
    T: AsRef<[u8]>,
{
    /// Build a new `PboVfs` from raw bytes and a parsed PBO header.
    ///
    /// If the PBO contains a `prefix` header extension (e.g. `DZ\AI`), all
    /// file paths are rooted under that prefix in the VFS tree.  This means
    /// multiple PBOs with different prefixes can be overlaid into a single
    /// coherent directory hierarchy that mirrors the game's virtual filesystem.
    pub fn new(source: T, pbo: PboFile) -> Self {
        let prefix = pbo.extensions.get("prefix").map(|p| p.replace('\\', "/")).unwrap_or_default();

        let entries = Self::build_entries(&pbo, &prefix);

        Self { inner: Arc::new(PboVfsInner { source, pbo, entries }) }
    }

    /// Return the parsed PBO metadata.
    pub fn pbo(&self) -> &PboFile {
        &self.inner.pbo
    }

    fn build_entries(pbo: &PboFile, prefix: &str) -> HashMap<String, VfsEntry> {
        let mut entries: HashMap<String, VfsEntry> = HashMap::new();

        // Seed the root directory.
        entries.insert(String::new(), VfsEntry::Directory { children: Vec::new() });

        // Create intermediate directories for the prefix itself (e.g. "DZ" then "DZ/AI").
        if !prefix.is_empty() {
            let prefix_parts: Vec<&str> = prefix.split('/').collect();
            for depth in 0..prefix_parts.len() {
                let parent_path =
                    if depth == 0 { String::new() } else { prefix_parts[..depth].join("/") };
                let child_name = prefix_parts[depth];
                let child_path = if parent_path.is_empty() {
                    child_name.to_string()
                } else {
                    format!("{parent_path}/{child_name}")
                };

                let parent = entries
                    .entry(parent_path)
                    .or_insert_with(|| VfsEntry::Directory { children: Vec::new() });
                if let VfsEntry::Directory { children } = parent
                    && !children.contains(&child_name.to_string())
                {
                    children.push(child_name.to_string());
                }

                entries
                    .entry(child_path)
                    .or_insert_with(|| VfsEntry::Directory { children: Vec::new() });
            }
        }

        for (idx, header) in pbo.entries.iter().enumerate() {
            // PBO paths use backslashes; normalise to forward slashes for VFS.
            let relative = header.filename.replace('\\', "/");

            // Prepend the prefix to get the full VFS path.
            let full_path =
                if prefix.is_empty() { relative } else { format!("{prefix}/{relative}") };

            // Register the file leaf.
            entries.insert(full_path.clone(), VfsEntry::File { entry_index: idx });

            // Ensure every ancestor directory exists and lists its children.
            let parts: Vec<&str> = full_path.split('/').collect();
            for depth in 0..parts.len() {
                let parent_path = if depth == 0 { String::new() } else { parts[..depth].join("/") };
                let child_name = parts[depth];
                let child_path = if parent_path.is_empty() {
                    child_name.to_string()
                } else {
                    format!("{parent_path}/{child_name}")
                };

                let parent = entries
                    .entry(parent_path)
                    .or_insert_with(|| VfsEntry::Directory { children: Vec::new() });

                if let VfsEntry::Directory { children } = parent
                    && !children.contains(&child_name.to_string())
                {
                    children.push(child_name.to_string());
                }

                if depth < parts.len() - 1 {
                    entries
                        .entry(child_path)
                        .or_insert_with(|| VfsEntry::Directory { children: Vec::new() });
                }
            }
        }

        entries
    }
}

impl<T> PboVfs<T>
where
    T: AsRef<[u8]>,
{
    fn lookup(&self, path: &str) -> vfs::VfsResult<&VfsEntry> {
        let key = path.strip_prefix('/').unwrap_or(path);
        self.inner.entries.get(key).ok_or_else(|| VfsError::from(VfsErrorKind::FileNotFound))
    }

    fn read_file_data(&self, entry_index: usize) -> Vec<u8> {
        let range = self.inner.pbo.entry_data_range_by_index(entry_index);
        self.inner.source.as_ref()[range].to_vec()
    }
}

impl<T> vfs::FileSystem for PboVfs<T>
where
    T: AsRef<[u8]> + Debug + Send + Sync + 'static,
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

    fn open_file(&self, path: &str) -> vfs::VfsResult<Box<dyn vfs::SeekAndRead + Send>> {
        let entry = self.lookup(path)?;
        match entry {
            VfsEntry::File { entry_index } => {
                let data = self.read_file_data(*entry_index);
                Ok(Box::new(Cursor::new(data)))
            }
            VfsEntry::Directory { .. } => Err(VfsError::from(VfsErrorKind::NotSupported)),
        }
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
                let header = &self.inner.pbo.entries[*entry_index];
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
    use async_trait::async_trait;
    use vfs::VfsResult;
    use vfs::async_vfs::AsyncFileSystem;
    use vfs::async_vfs::SeekAndRead;

    #[async_trait]
    impl<T> AsyncFileSystem for PboVfs<T>
    where
        T: AsRef<[u8]> + Debug + Send + Sync + 'static,
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
                    let data = self.read_file_data(*entry_index);
                    Ok(Box::new(AsyncCursor::new(data)))
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
