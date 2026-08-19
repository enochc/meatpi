use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// Socket-level failure: no route to the dongle, refused, reset.
    Io(std::io::Error),
    /// The adapter answered, but not with a usable payload.
    Adapter(String),
    /// The ECU is reachable but declined the request (`NO DATA`, `7F ...`).
    NoData,
    /// Nothing arrived before the read deadline.
    Timeout,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "i/o error: {e}"),
            Error::Adapter(m) => write!(f, "adapter error: {m}"),
            Error::NoData => write!(f, "no data from ECU"),
            Error::Timeout => write!(f, "timed out waiting for adapter"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        if matches!(
            e.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ) {
            Error::Timeout
        } else {
            Error::Io(e)
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
