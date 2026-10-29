//! Creates a Lua state whose `debug` library only the host can reach.
//!
//! mlua opens the debug library only through an `unsafe` constructor, because scripts that
//! hold it can break memory safety (`debug.setmetatable`, `debug.upvaluejoin` and friends).
//! This crate holds the one such call. It removes `debug` from the globals and from
//! `package.loaded` before it returns the state, so no script ever sees the library. The
//! host passes the returned table on to `lua-resume`, which reads suspended coroutines with it.
//!
//! The table also holds `unstartedfunction`, which the Lua debug library has no equivalent
//! for: the function a coroutine will run when first resumed. A coroutine that has not run has
//! no stack levels, so `debug.getinfo` and `debug.getlocal` cannot reach the function.

use mlua::{Function, Lua, LuaOptions, Result, StdLib, Table, Thread, ffi};

/// Creates a Lua state with the libraries in `libs` plus the debug library, which is removed
/// from the globals and from `package.loaded`. The debug table is returned to the host only.
pub fn new_lua_with_private_debug(libs: StdLib, options: LuaOptions) -> Result<(Lua, Table)> {
    // SAFETY: The debug library is the only unsafe-to-load library requested. It is taken out
    // of every place a script could find it (the globals and `package.loaded`) below, before
    // the state is returned and before any script runs, so scripts never get the table and the
    // host alone calls it.
    let lua = unsafe { Lua::unsafe_new_with(libs | StdLib::DEBUG, options) };
    let globals = lua.globals();
    let debug: Table = globals.get("debug")?;
    debug.set(
        "unstartedfunction",
        lua.create_function(unstarted_function)?,
    )?;
    globals.set("debug", mlua::Nil)?;
    if let Ok(package) = globals.get::<Table>("package")
        && let Ok(loaded) = package.get::<Table>("loaded")
    {
        loaded.set("debug", mlua::Nil)?;
    }
    Ok((lua, debug))
}

/// The function `thread` runs when first resumed, or `None` for a coroutine that has started
/// (it has stack levels, a yield in progress, or finished).
fn unstarted_function(lua: &Lua, thread: Thread) -> Result<Option<Function>> {
    // SAFETY: The closure runs on the Lua state `exec_raw` locks, with the thread value as its
    // only argument at the top of the stack. It reads the thread's stack without running it:
    // a coroutine that has no call levels, is not yielded and holds exactly one value holds
    // the function it will start with, which is copied to the calling state.
    unsafe {
        lua.exec_raw::<Option<Function>>(thread, |state| {
            let other = ffi::lua_tothread(state, -1);
            ffi::lua_pop(state, 1);
            if other.is_null()
                || ffi::lua_status(other) != ffi::LUA_OK
                || ffi::lua_gettop(other) != 1
                || ffi::lua_type(other, 1) != ffi::LUA_TFUNCTION
            {
                return;
            }
            let mut record: ffi::lua_Debug = std::mem::zeroed();
            if ffi::lua_getstack(other, 0, &raw mut record) == 0
                && ffi::lua_checkstack(other, 1) != 0
            {
                ffi::lua_pushvalue(other, 1);
                ffi::lua_xmove(other, state, 1);
            }
        })
    }
}
