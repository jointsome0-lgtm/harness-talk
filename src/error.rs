use std::{fmt, io};

/// Only fixed codes and class names cross a notification or discovery boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Coded(String),
    Class(&'static str),
}
impl Failure {
    pub fn coded(code: impl Into<String>) -> Self {
        Self::Coded(code.into())
    }
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Coded(code) => f.write_str(code),
            Self::Class(name) => f.write_str(name),
        }
    }
}
impl std::error::Error for Failure {}

#[derive(Debug)]
pub enum Error {
    Code(String),
    Value(String),
    Io(io::Error),
    Db(rusqlite::Error),
    Interrupted,
}
impl Error {
    pub fn code(code: impl Into<String>) -> Self {
        Self::Code(code.into())
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Code(s) | Self::Value(s) => f.write_str(s),
            Self::Io(e) => e.fmt(f),
            Self::Db(e) => e.fmt(f),
            Self::Interrupted => f.write_str("interrupted"),
        }
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}
impl From<Failure> for Error {
    fn from(e: Failure) -> Self {
        Self::Code(e.to_string())
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::code("invalid_json")
    }
}
