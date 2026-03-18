pub use parser::*;

#[cfg(feature = "async_vfs")]
pub mod async_pbo_vfs;
pub mod error;
mod parser;
#[cfg(feature = "vfs")]
pub mod pbo_vfs;
#[cfg(feature = "async_vfs")]
pub mod wrappers;
#[cfg(feature = "vfs")]
pub use vfs;
pub use winnow;
