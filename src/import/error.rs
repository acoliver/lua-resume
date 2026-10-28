//! Why a saved Lua 5.3 thread cannot be imported.

use crate::lua53::frame::MapError;

/// An import failure. Frame numbers count from the outermost frame, starting at 0.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ImportError {
    #[error("frame {frame}: {reason}")]
    Frame { frame: usize, reason: String },
    #[error("frame {frame} cannot be mapped onto the source: {source}")]
    Unmapped { frame: usize, source: MapError },
    #[error("frame {frame} holds {count} temporaries, which the import cannot restore")]
    Temporaries { frame: usize, count: usize },
    #[error("frame {frame} is a tail call, but its site in the source is not a `return`")]
    TailCallMismatch { frame: usize },
    #[error("{path}: no prototype {index}")]
    UnknownProto { path: String, index: usize },
    #[error("{path}: no chunk is registered for the source `{source_name}`")]
    UnknownChunk { path: String, source_name: String },
    #[error("{path}: the permanent `{name}` is not registered")]
    MissingPermanent { path: String, name: String },
    #[error("thread {thread}: {source}")]
    Thread {
        thread: usize,
        source: Box<ImportError>,
    },
    #[error("{path}: {reason}")]
    Malformed { path: String, reason: String },
}

impl ImportError {
    pub(super) fn frame(frame: usize, reason: impl Into<String>) -> Self {
        Self::Frame {
            frame,
            reason: reason.into(),
        }
    }

    pub(super) fn in_thread(thread: usize, source: Self) -> Self {
        Self::Thread {
            thread,
            source: Box::new(source),
        }
    }

    pub(super) fn malformed(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Malformed {
            path: path.into(),
            reason: reason.into(),
        }
    }
}
