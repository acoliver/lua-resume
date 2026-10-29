//! Why a coroutine cannot be captured. Errors name the path to the offending value.

/// A capture failure. `path` reads from the captured coroutine down to the value, for example
/// ``thread > level 2 > local `queue` > ["handler"] > upvalue `socket` ``.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("debug library call failed: {0}")]
    Lua(#[from] mlua::Error),
    #[error("{path}: the coroutine is {state}")]
    NotSuspended { path: String, state: &'static str },
    #[error("{path}: the top frame is `{found}`, not a yielding native function")]
    NotYielding { path: String, found: String },
    #[error("{path}: native frame `{name}` is not the yielding function")]
    NativeFrame { path: String, name: String },
    #[error("{path}: the frame has no integer `__lrs` site local; the code is not normalized")]
    NotNormalized { path: String },
    #[error("{path}: the function has no source position (stripped)")]
    Unidentifiable { path: String },
    #[error("{function}: several different functions share this chunk name and lines")]
    Ambiguous { function: String },
    #[error("{path}: holds a to-be-closed variable")]
    ToBeClosed { path: String },
    #[error("{path}: userdata that is not in the permanents")]
    Userdata { path: String },
    #[error("{path}: native function that is not in the permanents")]
    NativeFunction { path: String },
    #[error("{path}: unnamed stack value `{name}`")]
    Unnamed { path: String, name: String },
    #[error("the root name `{name}` is used twice")]
    DuplicateRoot { name: String },
    #[error("{path}: values of type {kind} cannot be saved")]
    Unsupported { path: String, kind: &'static str },
}

/// Why a value cannot be registered as a permanent.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PermanentError {
    #[error(
        "`{0}` has no identity: only tables, functions, threads and userdata can be permanents"
    )]
    NoIdentity(String),
    #[error("the name `{0}` is already used")]
    DuplicateName(String),
    #[error("the value is already named `{0}`")]
    AlreadyNamed(String),
    #[error("no permanent is named `{0}`")]
    UnknownName(String),
    #[error("the light userdata handle {0} is already used")]
    DuplicateHandle(u64),
    #[error("the light userdata already has the handle {0}")]
    AlreadyHandled(u64),
}
