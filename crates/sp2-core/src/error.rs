use crate::PixelFormat;

/// Result alias used across `sp2-rs`.
pub type Result<T> = core::result::Result<T, Error>;

/// Error type shared by every backend.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The requested operation is not available on the current platform or
    /// graphics backend.
    #[error("unsupported: {0}")]
    Unsupported(&'static str),

    /// No sender / server matched the request.
    #[error("sender not found: {0}")]
    NotFound(String),

    /// The receiver is not connected to a sender.
    #[error("not connected to a sender")]
    NotConnected,

    /// The caller passed an invalid argument.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// The pixel format is not supported by the backend.
    #[error("unsupported pixel format: {0:?}")]
    InvalidFormat(PixelFormat),

    /// The provided buffer does not match the expected size.
    #[error("size mismatch: expected {expected} bytes, got {actual} bytes")]
    SizeMismatch {
        /// Number of bytes the backend expected.
        expected: usize,
        /// Number of bytes actually provided.
        actual: usize,
    },

    /// A lock or wait timed out (for example the Spout texture access mutex).
    #[error("timed out")]
    Timeout,

    /// An operating system or graphics API call failed.
    #[error("os error {code:#x}: {message}")]
    Os {
        /// Native error code (`HRESULT`, `GetLastError`, `kern_return_t`, ...).
        code: i64,
        /// Human readable description.
        message: String,
    },

    /// Backend specific failure that does not map to the variants above.
    #[error("{0}")]
    Backend(String),
}

impl Error {
    /// Convenience constructor for [`Error::Os`].
    pub fn os(code: impl Into<i64>, message: impl Into<String>) -> Self {
        Error::Os {
            code: code.into(),
            message: message.into(),
        }
    }

    /// Convenience constructor for [`Error::Backend`].
    pub fn backend(message: impl Into<String>) -> Self {
        Error::Backend(message.into())
    }
}
