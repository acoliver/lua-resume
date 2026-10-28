//! Why a snapshot cannot be restored.

use crate::continuation::ContinuationError;

/// A restore failure.
#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("lua failed while restoring: {0}")]
    Lua(#[from] mlua::Error),
    #[error("the permanent `{0}` is not registered")]
    MissingPermanent(String),
    #[error("the light userdata with handle {0} is not registered")]
    MissingLightUserData(u64),
    #[error("no source for chunk `{0}`")]
    UnknownChunk(String),
    #[error("no function defined at {chunk}:{line}-{end_line}")]
    UnknownFunction {
        chunk: String,
        line: u32,
        end_line: u32,
    },
    #[error("{function} no longer matches the saved closure: {reason}")]
    SourceChanged { function: String, reason: String },
    #[error("cannot continue {function}: {source}")]
    Continuation {
        function: String,
        source: ContinuationError,
    },
    #[error("the snapshot is inconsistent: {0}")]
    Malformed(String),
}
