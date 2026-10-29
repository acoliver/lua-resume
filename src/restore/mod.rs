//! Restores a [`Snapshot`] as a suspended coroutine.
//!
//! Values are rebuilt first: tables with their identity, cycles and metatables, Lua closures
//! from the function the game's own source defines at the saved identity, upvalue cells shared
//! through `debug.upvaluejoin`, and permanents by name. Then each coroutine gets its frames,
//! outermost first: the continuation of every frame is generated from the source, bound to the
//! saved closure's cells and given the saved locals and varargs. Each frame's `__lr_next` runs
//! the next inner frame, and the innermost one is the native function the original waited in.
//! The new coroutine is resumed once, so it stops in that native function as the original did,
//! and the next resume with some values behaves like resuming the original with them.
//!
//! [`restore_all`] rebuilds the roots and every coroutine of a snapshot from one graph.
//! [`check_restore`] lists every refusal a restore would hit, for a host that wants to see all
//! the gaps in a large save at once.
//!
//! Upvalues are bound by name. A closure restored from a Lua 5.3 save or from a capture takes
//! each upvalue of the rebuilt function from the saved cell of the same name, whatever order
//! the two compiles number them in. Equal names pair in order. `_ENV` that the save lacks stays
//! the environment of the chunk the function is loaded in; any other name that only one side
//! has is refused.
//!
//! The code comes from [`Sources`], the host's trusted normalized chunks; a snapshot supplies
//! only data and names. A snapshot that names a function, chunk or permanent the host does
//! not have, or whose function no longer matches the source, is refused.

mod build;
mod check;
mod error;
mod frames;
mod sources;

pub use check::{Refusal, check_restore};
pub use error::RestoreError;
pub use sources::Sources;

use mlua::{Function, Lua, Thread, Value};

use crate::capture::{Debug, Permanents, Snapshot};
use crate::continuation::PROTECTED_NEXT;

/// What [`restore_all`] rebuilt from one snapshot. All of it shares one graph.
pub struct Restored {
    /// The named roots, in the order they were captured.
    pub roots: Vec<(String, Value)>,
    /// The coroutines that were captured, in the order they were given.
    pub threads: Vec<Thread>,
}

/// Restores `snapshot`, whose first thread is the coroutine that was captured.
pub fn restore(
    lua: &Lua,
    debug: &Debug,
    snapshot: &Snapshot,
    sources: &Sources,
    permanents: &Permanents,
) -> Result<Thread, RestoreError> {
    rebuild(lua, debug, snapshot, sources, permanents)?.first_thread()
}

/// Restores every root and coroutine of `snapshot` once, so tables, closures, upvalue cells
/// and coroutines shared in the original are shared in the result.
pub fn restore_all(
    lua: &Lua,
    debug: &Debug,
    snapshot: &Snapshot,
    sources: &Sources,
    permanents: &Permanents,
) -> Result<Restored, RestoreError> {
    rebuild(lua, debug, snapshot, sources, permanents)?.restored()
}

fn rebuild<'a>(
    lua: &'a Lua,
    debug: &'a Debug,
    snapshot: &'a Snapshot,
    sources: &'a Sources,
    permanents: &'a Permanents,
) -> Result<build::Restorer<'a>, RestoreError> {
    let mut restorer = build::Restorer::new(lua, debug, snapshot, sources, permanents)?;
    restorer.build_objects()?;
    restorer.fill()?;
    restorer.build_threads()?;
    Ok(restorer)
}

/// The functions every restore needs: `maker` builds the `__lr_next` of a frame and `entry`
/// builds the body of a coroutine whose frames are installed later.
pub(crate) struct Helpers {
    pub(crate) maker: Function,
    pub(crate) entry: Function,
    pub(crate) cell: Function,
    pub(crate) protect: Function,
}

const HELPERS: &str = "local unpack = table.unpack
local function maker(f, vals, nxt, varargs, n)
  return function() return f(vals, nxt, unpack, unpack(varargs, 1, n)) end
end
local function entry(slot)
  return function() return slot.start() end
end
local function cell()
  local v
  return function() return v end
end
return maker, entry, cell";

impl Helpers {
    pub(crate) fn load(lua: &Lua) -> mlua::Result<Self> {
        let (maker, entry, cell): (Function, Function, Function) =
            lua.load(HELPERS).set_name("=lua-resume").eval()?;
        let protect = lua.load(PROTECTED_NEXT).set_name("=lua-resume").eval()?;
        Ok(Self {
            maker,
            entry,
            cell,
            protect,
        })
    }
}
