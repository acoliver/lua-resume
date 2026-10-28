//! Captures a coroutine suspended in normalized code as a [`Snapshot`].
//!
//! The coroutine's frames are read with the host's [`Debug`] capability: for each Lua frame
//! its function (chunk name and lines), the call site in `__lrs`, every named local and the
//! varargs. The top frame must be the yielding native function. Values become data: tables
//! are shared by identity with cycles kept and metatables recorded, closures are their
//! function plus upvalue cells shared by `debug.upvalueid`, and coroutines reachable from the
//! stack are captured as well. Anything in the [`Permanents`] is saved by name. The snapshot
//! never holds bytecode.
//!
//! Capture refuses, with an error that names the path to the value: a coroutine that is not
//! suspended in a yielding native function, a native frame below the top, a frame that is not
//! normalized, ambiguous function identities, unnamed stack values, native functions and
//! userdata that are not permanents, and locals that may be to-be-closed variables (a table
//! with a `__close` metamethod).

mod debug;
mod error;
mod frames;
mod path;
mod permanents;
mod snapshot;
mod values;

pub use debug::{Debug, Info};
pub use error::{CaptureError, PermanentError};
pub use permanents::Permanents;
pub use snapshot::{
    Callee, Cell, ClosureData, Frame, FunctionId, Local, Snapshot, TableData, ThreadData,
    UpvalueRef, Value,
};

use std::collections::HashMap;

use mlua::{Lua, Thread};

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
    let mut capturer = Capturer::new(lua, debug, permanents)?;
    capturer.thread_id(thread, &Path::root("thread"));
    capturer.run()?;
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
    debug: &'a Debug,
    permanents: &'a Permanents,
    yield_function: usize,
    snapshot: Snapshot,
    tables: HashMap<usize, usize>,
    closures: HashMap<usize, usize>,
    cells: HashMap<usize, usize>,
    threads: HashMap<usize, usize>,
    shapes: HashMap<FunctionId, Shape>,
    work: Vec<Work>,
}

impl<'a> Capturer<'a> {
    fn new(lua: &Lua, debug: &'a Debug, permanents: &'a Permanents) -> Result<Self, CaptureError> {
        let coroutine: Option<mlua::Table> = lua.globals().get("coroutine")?;
        let yield_function = match coroutine {
            Some(table) => table
                .get::<Option<mlua::Function>>("yield")?
                .map_or(0, |function| function.to_pointer().addr()),
            None => 0,
        };
        Ok(Self {
            debug,
            permanents,
            yield_function,
            snapshot: Snapshot::default(),
            tables: HashMap::new(),
            closures: HashMap::new(),
            cells: HashMap::new(),
            threads: HashMap::new(),
            shapes: HashMap::new(),
            work: Vec::new(),
        })
    }
}
