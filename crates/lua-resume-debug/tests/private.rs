//! Scripts cannot reach the debug library; the host can.

use lua_resume_debug::new_lua_with_private_debug;
use mlua::{Function, LuaOptions, StdLib, Table, Value};

fn state() -> (mlua::Lua, Table) {
    new_lua_with_private_debug(StdLib::ALL_SAFE, LuaOptions::default()).unwrap()
}

#[test]
fn scripts_find_no_debug_library() {
    let (lua, _) = state();
    let found: Vec<bool> = lua
        .load(
            "return {debug == nil, package.loaded.debug == nil, rawget(_G, 'debug') == nil, \
             (select(1, pcall(require, 'debug'))) == false}",
        )
        .eval()
        .unwrap();
    assert_eq!(found, [true, true, true, true]);
}

#[test]
fn the_host_receives_a_working_debug_table() {
    let (lua, debug) = state();
    let getinfo: Function = debug.get("getinfo").unwrap();
    let probe: Function = lua.load("return function() end").eval().unwrap();
    let info: Table = getinfo.call((probe, "S")).unwrap();
    assert_eq!(info.get::<String>("what").unwrap(), "Lua");
}

#[test]
fn other_libraries_stay_available() {
    let (lua, _) = state();
    let value: Value = lua.load("return string.rep('a', 2)").eval().unwrap();
    assert_eq!(value.as_str().unwrap(), "aa");
}

#[test]
fn a_state_without_the_package_library_still_works() {
    let (lua, debug) = new_lua_with_private_debug(StdLib::STRING, LuaOptions::default()).unwrap();
    assert!(lua.globals().get::<Value>("debug").unwrap().is_nil());
    assert!(debug.contains_key("getlocal").unwrap());
}

#[test]
fn the_function_of_a_coroutine_that_has_not_run_is_readable_by_the_host() {
    let (lua, debug) = state();
    let unstarted: Function = debug.get("unstartedfunction").unwrap();
    let body: Function = lua
        .load("return function(...) coroutine.yield(...) end")
        .eval()
        .unwrap();
    let thread = lua.create_thread(body.clone()).unwrap();
    let found: Option<Function> = unstarted.call(thread.clone()).unwrap();
    assert_eq!(found.unwrap(), body);
    assert_eq!(thread.status(), mlua::ThreadStatus::Resumable);

    thread.resume::<()>(1).unwrap();
    assert!(
        unstarted
            .call::<Option<Function>>(thread.clone())
            .unwrap()
            .is_none()
    );
    thread.resume::<()>(()).unwrap();
    assert!(
        unstarted
            .call::<Option<Function>>(thread)
            .unwrap()
            .is_none()
    );
}
