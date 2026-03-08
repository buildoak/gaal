use thiserror::Error;

/// Errors returned by Gaal operations.
#[derive(Debug, Error)]
pub enum GaalError {
    /// No matching results were found.
    #[error("no results")]
    NoResults,
    /// The provided ID matched multiple sessions.
    #[error("ambiguous id: {0}")]
    AmbiguousId(String),
    /// The requested entity was not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// The SQLite index does not exist yet.
    #[error("index not found; run `gaal index backfill`")]
    NoIndex,
    /// Parsing failed for user input or source data.
    #[error("parse error: {0}")]
    ParseError(String),
    /// Filesystem I/O failure.
    #[error(transparent)]
    Io(std::io::Error),
    /// SQLite database failure.
    #[error(transparent)]
    Db(rusqlite::Error),
    /// Internal logic error (e.g. serialization, data format).
    #[error("{0}")]
    Internal(String),
    /// Invalid configuration or config loading failure.
    #[error("config error: {0}")]
    Config(String),
    /// Catch-all error variant for propagated anyhow errors.
    #[error(transparent)]
    Other(anyhow::Error),
}

impl GaalError {
    /// Returns the process exit code associated with this error.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::NoResults => 1,
            Self::AmbiguousId(_) => 2,
            Self::NotFound(_) => 3,
            Self::NoIndex => 10,
            Self::ParseError(_) => 11,
            Self::Io(_) | Self::Db(_) | Self::Internal(_) | Self::Config(_) | Self::Other(_) => 1,
        }
    }
}

impl From<rusqlite::Error> for GaalError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Db(value)
    }
}

impl From<std::io::Error> for GaalError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<anyhow::Error> for GaalError {
    fn from(value: anyhow::Error) -> Self {
        Self::Other(value)
    }
}
