//! Creates a Lua state whose `debug` library only the host can reach.
//!
//! mlua opens the debug library only through an `unsafe` constructor, because scripts that
//! hold it can break memory safety (`debug.setmetatable`, `debug.upvaluejoin` and friends).
//! This crate holds the one such call. It removes `debug` from the globals and from
//! `package.loaded` before it returns the state, so no script ever sees the library. The
//! host passes the returned table on to `lua-resume`, which reads suspended coroutines with it.

use mlua::{Lua, LuaOptions, Result, StdLib, Table};

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
    globals.set("debug", mlua::Nil)?;
    if let Ok(package) = globals.get::<Table>("package")
        && let Ok(loaded) = package.get::<Table>("loaded")
    {
        loaded.set("debug", mlua::Nil)?;
    }
    Ok((lua, debug))
}
