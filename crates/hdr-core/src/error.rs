//! Error type shared by every engine entry point.

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, HdrError>;

#[derive(Debug, thiserror::Error)]
pub enum HdrError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("cannot decode {path}: {source}")]
    Decode {
        path: PathBuf,
        #[source]
        source: ::image::ImageError,
    },

    #[error("unsupported format for {0} (expected tiff, png, jpeg or exr)")]
    UnsupportedFormat(PathBuf),

    #[error("no frames to merge")]
    NoFrames,

    #[error("frame {path} is {got_w}x{got_h}, expected {want_w}x{want_h}")]
    SizeMismatch {
        path: PathBuf,
        want_w: u32,
        want_h: u32,
        got_w: u32,
        got_h: u32,
    },

    #[error("invalid parameter: {0}")]
    Invalid(String),

    #[error("{0}")]
    Engine(String),
}
