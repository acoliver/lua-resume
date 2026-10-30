//! The data a capture produces: frames, and the tables, closures and upvalue cells they reach.
//!
//! A snapshot holds only data and names. Objects that can be shared (tables, closures,
//! upvalue cells, coroutines) live in arenas and refer to each other by index, so identity,
//! cycles and shared upvalues survive serialization. Indices are assigned in discovery order
//! and table entries are ordered by key, so capturing equal state gives equal snapshots.

use serde::{Deserialize, Serialize};

use crate::continuation::{FunctionKey, FunctionRef, FunctionShape};

/// The function a frame or closure runs: its chunk name and the lines its definition spans.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FunctionId {
    pub chunk: String,
    pub line: u32,
    pub end_line: u32,
    /// Tells this function from others on the same lines. Capture records it; saves from
    /// before it existed and imports of Lua 5.3 saves leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<FunctionShape>,
}

impl FunctionId {
    /// The reference restore finds the function by: its lines and, when saved, its shape.
    pub fn function_ref(&self) -> FunctionRef {
        FunctionRef {
            key: self.key(),
            shape: self.shape,
        }
    }

    /// The key `continuation::continuation_source` finds the function by.
    pub fn key(&self) -> FunctionKey {
        FunctionKey {
            line: self.line,
            end_line: self.end_line,
        }
    }
}

/// A Lua value as data. Floats are stored as their exact bits, so NaN payloads, infinities
/// and negative zero survive. Strings are exact bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Value {
    Nil,
    Boolean(bool),
    Integer(i64),
    Float(u64),
    String(Vec<u8>),
    /// An index into [`Snapshot::tables`].
    Table(usize),
    /// An index into [`Snapshot::closures`].
    Closure(usize),
    /// An index into [`Snapshot::threads`].
    Thread(usize),
    /// A value the host named in its permanents, such as a native function or a userdata.
    Permanent(String),
    /// A light userdata, by the handle the host registered it under with
    /// [`Permanents::insert_light`](super::Permanents::insert_light).
    LightUserData(u64),
}

impl Value {
    /// A float value from `number`.
    pub fn float(number: f64) -> Self {
        Self::Float(number.to_bits())
    }
}

/// A named stack slot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Local {
    pub name: String,
    pub value: Value,
}

/// The call a frame is waiting in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Callee {
    /// A native function the host named, such as `coroutine.yield`.
    Native(String),
    /// The next inner frame.
    Lua(FunctionId),
    /// `xpcall(f, handler, ...)`: the next inner frame runs inside it, with this message handler.
    /// A frame waiting in `pcall` is `Native("pcall")` followed by the inner frame.
    Xpcall { handler: Value },
}

/// A local of a frame that is the same variable as an upvalue cell: closures that use the
/// cell read and write the frame's local, and a restore binds them to it again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedLocal {
    /// An index into [`Frame::locals`].
    pub local: usize,
    /// An index into [`Snapshot::cells`].
    pub cell: usize,
}

/// A suspended Lua function call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub function: FunctionId,
    /// The closure that is running, an index into [`Snapshot::closures`].
    pub closure: usize,
    /// The call site recorded in the frame's `__lrs` local.
    pub site: u32,
    /// The source line the frame is suspended at.
    pub line: i64,
    pub tail_call: bool,
    /// Every named local in stack order, including generated ones.
    pub locals: Vec<Local>,
    pub varargs: Vec<Value>,
    pub pending: Callee,
    /// The locals that closures share through an upvalue cell.
    #[serde(default)]
    pub shared: Vec<SharedLocal>,
}

/// The metatable of a table: another table of the snapshot, or a permanent the host names.
///
/// A snapshot written before permanent metatables existed holds a bare index, which loads as
/// [`Metatable::Table`]. A name is written as a string. The engine saves class tables that
/// way, because it names them and does not enter them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Metatable {
    /// An index into [`Snapshot::tables`].
    Table(usize),
    /// The name of a table in the host's permanents.
    Permanent(String),
}

/// A table's entries in key order and its metatable.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableData {
    pub entries: Vec<(Value, Value)>,
    pub metatable: Option<Metatable>,
}

/// An upvalue of a closure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpvalueRef {
    pub name: String,
    /// An index into [`Snapshot::cells`]. Closures that share an upvalue share its cell.
    pub cell: usize,
}

/// A Lua closure: its function and the cells of its upvalues.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosureData {
    pub function: FunctionId,
    pub upvalues: Vec<UpvalueRef>,
}

/// The storage behind one or more upvalues.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub value: Value,
}

/// A coroutine reachable from the captured one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreadData {
    /// Suspended in normalized code; frames run from the outermost call to the innermost.
    Suspended { frames: Vec<Frame> },
    /// Created and never resumed: it starts by calling this function, a closure or permanent,
    /// with the arguments of its first resume. The caller supplies those, so none are saved.
    Unstarted { function: Value },
    /// Finished, or ended by an error.
    Dead,
}

/// Coroutines, named roots and everything they reach, as one graph. Every table, closure,
/// cell and coroutine appears once, so values shared between coroutines and roots are shared
/// after a restore. A single capture has `threads[0]` as the captured coroutine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub threads: Vec<ThreadData>,
    pub tables: Vec<TableData>,
    pub closures: Vec<ClosureData>,
    pub cells: Vec<Cell>,
    /// Named data roots, in the order the host gave them.
    #[serde(default)]
    pub roots: Vec<(String, Value)>,
    /// The coroutines the host asked to capture, as indices into `threads`, in the order the
    /// host gave them. Other entries of `threads` were reached through values.
    #[serde(default)]
    pub captured: Vec<usize>,
}

impl Snapshot {
    /// The frames of the captured coroutine, outermost first.
    pub fn frames(&self) -> &[Frame] {
        match self.threads.first() {
            Some(ThreadData::Suspended { frames }) => frames,
            _ => &[],
        }
    }
}
