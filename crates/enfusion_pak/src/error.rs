use thiserror::Error;
use winnow::error::ContextError;
use winnow::error::StrContext;

#[derive(Debug, Error)]
pub enum PakError {
    #[error("I/O error occurred")]
    IoError(#[from] std::io::Error),

    #[error("Parser error")]
    ParserError(ContextError<StrContext>),
}
