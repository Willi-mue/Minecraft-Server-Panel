//! One error type for the whole panel. The variants mirror the kinds of failure the API maps to
//! HTTP status codes (see `api::ApiError`).

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// Another action is running (HTTP 409)
    Busy(String),
    /// Bad input or an action that can't be done now (HTTP 400)
    Invalid(String),
    /// Missing file / unknown player / nothing found (HTTP 404)
    NotFound(String),
    /// Target exists already (HTTP 409)
    Exists(String),
    /// A file is locked by the running server (Windows) (HTTP 409)
    InUse(String),
    /// A remote service failed or answered with an error (HTTP 502)
    Remote(String),
    /// HTTP error status from a remote service
    Status(u16, String),
    /// Anything else from the file system
    Io(std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Busy(s)
            | Error::Invalid(s)
            | Error::NotFound(s)
            | Error::Exists(s)
            | Error::InUse(s)
            | Error::Remote(s) => f.write_str(s),
            Error::Status(code, url) => write!(f, "HTTP {code} from {url}"),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        if is_in_use(&e) {
            return Error::InUse(e.to_string());
        }
        Error::Io(e)
    }
}

impl From<zip::result::ZipError> for Error {
    fn from(e: zip::result::ZipError) -> Self {
        match e {
            zip::result::ZipError::Io(io) => io.into(),
            other => Error::Invalid(other.to_string()),
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Invalid(e.to_string())
    }
}

/// Windows keeps files of a running server locked (sharing / lock violation, access denied).
pub fn is_in_use(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::PermissionDenied
        || matches!(e.raw_os_error(), Some(32 | 33) if cfg!(windows))
}

/// `Err(Error::Invalid(..))` from a format string.
#[macro_export]
macro_rules! invalid {
    ($($arg:tt)*) => { Err($crate::error::Error::Invalid(format!($($arg)*))) };
}

#[macro_export]
macro_rules! not_found {
    ($($arg:tt)*) => { Err($crate::error::Error::NotFound(format!($($arg)*))) };
}
