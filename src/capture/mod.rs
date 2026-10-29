//! Captures a coroutine suspended in normalized code as a [`Snapshot`].
//!
//! A coroutine that was created and never resumed has no frames; it is captured as the function
//! it starts with, read with the debug table's `unstartedfunction`, and a restore creates a
//! coroutine from that function. Arguments of its first resume are the caller's.
//!
//! The coroutine's frames are read with the host's [`Debug`] capability: for each Lua frame
//! its function (chunk name and lines), the call site in `__lrs`, every named local and the
//! varargs. The top frame must be the yielding native function. Values become data: tables
//! are shared by identity with cycles kept and metatables recorded, closures are their
//! function plus upvalue cells shared by `debug.upvalueid`, and coroutines reachable from the
//! stack are captured as well. Anything in the [`Permanents`] is saved by name. The snapshot
//! never holds bytecode.
//!
//! [`capture_all`] captures several coroutines and named data roots as one graph, so values
//! they share stay shared after a restore.
//!
//! Capture refuses, with an error that names the path to the value: a coroutine that is not
//! suspended in a yielding native function, a native frame below the top other than a `pcall`
//! or `xpcall` between two Lua frames (saved as the pending call of the outer frame, with the
//! message handler of an `xpcall`), a frame that is not
//! normalized, ambiguous function identities, unnamed stack values, native functions and
//! userdata that are not permanents, and locals that may be to-be-closed variables (a table
//! with a `__close` metamethod).

mod alias;
mod debug;
mod error;
mod frames;
mod origins;
mod path;
mod permanents;
mod snapshot;
mod values;

pub use debug::{Debug, Info};
pub use error::{CaptureError, PermanentError};
pub use permanents::Permanents;
pub use snapshot::{
    Callee, Cell, ClosureData, Frame, FunctionId, Local, Metatable, SharedLocal, Snapshot,
    TableData, ThreadData, UpvalueRef, Value,
};

use std::collections::{HashMap, HashSet};

use mlua::{Lua, Thread};

pub(crate) use origins::{
    Origin, register as register_origin, register_protector, register_relocation,
};
use path::Path;
use values::Work;

/// Captures `thread`, which must be suspended in a yielding native function called from
/// normalized code.
pub fn capture(
    lua: &Lua,
    debug: &Debug,
    thread: &Thread,
    permanents: &Permanents,
) -> Result<Snapshot, CaptureError> {
    capture_all(lua, debug, &[], std::slice::from_ref(thread), permanents)
}

/// Captures `threads`, each suspended in a yielding native function called from normalized
/// code, and the named data `roots`, as one graph. A table, closure, upvalue cell or coroutine
/// reached from several threads or roots is captured once, so a restore keeps it shared.
///
/// Listed threads get the first indices of [`Snapshot::threads`] in the order given and
/// [`Snapshot::captured`] repeats that order. Roots keep the order given.
pub fn capture_all(
    lua: &Lua,
    debug: &Debug,
    roots: &[(&str, mlua::Value)],
    threads: &[Thread],
    permanents: &Permanents,
) -> Result<Snapshot, CaptureError> {
    let mut capturer = Capturer::new(lua, debug, permanents)?;
    for (index, thread) in threads.iter().enumerate() {
        let label = if threads.len() == 1 {
            "thread".to_owned()
        } else {
            format!("thread {index}")
        };
        let id = capturer.thread_id(thread, &Path::root(&label));
        capturer.snapshot.captured.push(id);
    }
    capturer.listed = capturer.snapshot.threads.len();
    capturer.roots(roots)?;
    capturer.run()?;
    capturer.find_aliases()?;
    Ok(capturer.snapshot)
}

/// The shape of a function, which tells different functions with equal lines apart.
#[derive(PartialEq, Eq)]
struct Shape {
    params: i64,
    vararg: bool,
    upvalues: i64,
    active_lines: Vec<i64>,
}

/// Capture state: the snapshot being built and the identities seen so far.
struct Capturer<'a> {
    lua: &'a Lua,
    debug: &'a Debug,
    permanents: &'a Permanents,
    yield_function: usize,
    /// The addresses of the host's `pcall` and `xpcall`.
    pcall_function: usize,
    xpcall_function: usize,
    /// Functions a restore built that call the next frame in a protected call.
    protectors: HashSet<usize>,
    /// Threads below this index were asked for and must be suspended.
    listed: usize,
    snapshot: Snapshot,
    tables: HashMap<usize, usize>,
    closures: HashMap<usize, usize>,
    /// The closures by snapshot index.
    functions: Vec<mlua::Function>,
    /// The frames whose locals are checked for sharing a cell with a closure.
    probes: Vec<alias::Probe>,
    cells: HashMap<usize, usize>,
    threads: HashMap<usize, usize>,
    shapes: HashMap<FunctionId, Shape>,
    origins: HashMap<usize, Origin>,
    relocations: HashMap<FunctionId, Option<FunctionId>>,
    work: Vec<Work>,
}

impl<'a> Capturer<'a> {
    fn new(
        lua: &'a Lua,
        debug: &'a Debug,
        permanents: &'a Permanents,
    ) -> Result<Self, CaptureError> {
        let coroutine: Option<mlua::Table> = lua.globals().get("coroutine")?;
        let yield_function = match coroutine {
            Some(table) => table
                .get::<Option<mlua::Function>>("yield")?
                .map_or(0, |function| function.to_pointer().addr()),
            None => 0,
        };
        Ok(Self {
            lua,
            debug,
            permanents,
            yield_function,
            pcall_function: native_address(lua, permanents, "pcall"),
            xpcall_function: native_address(lua, permanents, "xpcall"),
            protectors: origins::protectors(lua),
            listed: 0,
            snapshot: Snapshot::default(),
            tables: HashMap::new(),
            closures: HashMap::new(),
            functions: Vec::new(),
            probes: Vec::new(),
            cells: HashMap::new(),
            threads: HashMap::new(),
            shapes: HashMap::new(),
            origins: origins::all(lua),
            relocations: origins::relocations(lua),
            work: Vec::new(),
        })
    }
}

/// The address of the host's global function `name`: the permanent of that name when there
/// is one, else the global as it is now. Zero when the host has none.
fn native_address(lua: &Lua, permanents: &Permanents, name: &str) -> usize {
    let value = permanents.get(name).cloned().or_else(|| {
        lua.globals()
            .get::<Option<mlua::Value>>(name)
            .ok()
            .flatten()
    });
    match value {
        Some(mlua::Value::Function(function)) => function.to_pointer().addr(),
        _ => 0,
    }
}
