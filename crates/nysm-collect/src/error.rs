use std::fmt;

use nysm_core::Status;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectError {
    Unsupported(String),
    PermissionDenied(String),
    /// The target disappeared mid-read (e.g. process exited).
    Gone,
    Failed(String),
}

pub type CResult<T> = Result<T, CollectError>;

impl CollectError {
    pub fn status(&self) -> Status {
        match self {
            CollectError::Unsupported(_) => Status::Unsupported,
            CollectError::PermissionDenied(_) => Status::PermissionDenied,
            CollectError::Gone | CollectError::Failed(_) => Status::CollectionError,
        }
    }

    pub fn reason(&self) -> String {
        self.to_string()
    }

    /// Map an I/O error reading `what`.
    pub fn from_io(what: &str, e: &std::io::Error) -> Self {
        use std::io::ErrorKind::*;
        match e.kind() {
            NotFound => CollectError::Unsupported(format!("{what} not present")),
            PermissionDenied => {
                CollectError::PermissionDenied(format!("{what}: permission denied"))
            }
            _ if e.raw_os_error() == Some(3) => CollectError::Gone, // ESRCH
            _ => CollectError::Failed(format!("{what}: {e}")),
        }
    }
}

impl fmt::Display for CollectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CollectError::Unsupported(s)
            | CollectError::PermissionDenied(s)
            | CollectError::Failed(s) => f.write_str(s),
            CollectError::Gone => f.write_str("target no longer exists"),
        }
    }
}

impl std::error::Error for CollectError {}

impl<T> From<CollectError> for nysm_core::Reading<T> {
    fn from(e: CollectError) -> Self {
        nysm_core::Reading::missing(e.status(), e.reason())
    }
}
