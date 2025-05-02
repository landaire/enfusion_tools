use thiserror::Error;

#[derive(Debug, Error)]
pub enum PakError {
    #[error("I/O error occurred")]
    IoError(#[from] std::io::Error),

    #[error("Unknown fourcc encountered: {0:?}")]
    UnknownChunk([u8; 4]),
}
