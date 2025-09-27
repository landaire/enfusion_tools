#![doc = include_str!("../README.md")]

pub use parser::*;

/// Async VFS support
#[cfg(feature = "async_vfs")]
pub mod async_pak_vfs;
pub mod error;
/// VFS support
#[cfg(feature = "vfs")]
pub mod pak_vfs;
mod parser;
#[cfg(any(feature = "vfs", feature = "async_vfs"))]
pub use vfs;
pub use winnow;
pub mod wrappers;
