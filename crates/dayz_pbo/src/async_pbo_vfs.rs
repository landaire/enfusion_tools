use std::ops::Range;

use async_trait::async_trait;
use vfs::VfsError;

/// Trait for asynchronously reading a byte range from a data source.
///
/// This is intentionally identical to `enfusion_pak::async_pak_vfs::AsyncReadAt`
/// but defined independently so that `dayz_pbo` does not depend on `enfusion_pak`.
#[async_trait]
pub trait AsyncReadAt {
    /// Read the bytes in `file_range` and return them.
    async fn read_at(&self, file_range: Range<usize>) -> Result<impl AsRef<[u8]>, VfsError>;
}
