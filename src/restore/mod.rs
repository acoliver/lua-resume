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
//! The code comes from [`Sources`], the host's trusted normalized chunks; a snapshot supplies
//! only data and names. A snapshot that names a function, chunk or permanent the host does
//! not have, or whose function no longer matches the source, is refused.

mod build;
mod error;
mod frames;
mod sources;

pub use error::RestoreError;
pub use sources::Sources;

use mlua::{Function, Lua, Thread};

use crate::capture::{Debug, Permanents, Snapshot};

/// Restores `snapshot`, whose first thread is the coroutine that was captured.
pub fn restore(
    lua: &Lua,
    debug: &Debug,
    snapshot: &Snapshot,
    sources: &Sources,
    permanents: &Permanents,
) -> Result<Thread, RestoreError> {
    let mut restorer = build::Restorer::new(lua, debug, snapshot, sources, permanents)?;
    restorer.build_objects()?;
    restorer.fill()?;
    restorer.build_threads()?;
    restorer.first_thread()
}

/// The functions every restore needs: `maker` builds the `__lr_next` of a frame and `entry`
/// builds the body of a coroutine whose frames are installed later.
pub(crate) struct Helpers {
    pub(crate) maker: Function,
    pub(crate) entry: Function,
    pub(crate) cell: Function,
    pub(crate) protect: Function,
}

const HELPERS: &str = "local unpack, pcall = table.unpack, pcall
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
local function protect(inner)
  return function() return pcall(inner) end
end
return maker, entry, cell, protect";

impl Helpers {
    pub(crate) fn load(lua: &Lua) -> mlua::Result<Self> {
        let (maker, entry, cell, protect): (Function, Function, Function, Function) =
            lua.load(HELPERS).set_name("=lua-resume").eval()?;
        Ok(Self {
            maker,
            entry,
            cell,
            protect,
        })
    }
}
