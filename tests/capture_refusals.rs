//! Capture refuses what a save cannot hold, with an error that names the path to it.

mod capture_support;

use capture_support::{Host, host};
use lua_resume::capture::{CaptureError, Permanents, Snapshot};
use mlua::{Thread, ThreadStatus, Value};

fn raw(host: &Host, source: &str, name: &str) {
    host.lua
        .load(source)
        .set_name(format!("={name}"))
        .exec()
        .unwrap();
}

fn refused(host: &Host, thread: &Thread) -> CaptureError {
    host.capture(thread).expect_err("capture should be refused")
}

#[test]
fn to_be_closed_variables_are_refused() {
    let host = host();
    host.run(
        "function f()
  local guard <close> = setmetatable({}, {__close = function() end})
  coroutine.yield()
end
",
        "close.lua",
    );
    let error = refused(&host, &host.started("f", ()));
    let CaptureError::ToBeClosed { path } = error else {
        panic!("unexpected {error}");
    };
    assert_eq!(path, "thread > level 1 > local `guard`");
}

#[test]
fn userdata_that_is_not_a_permanent_is_refused_with_its_path() {
    let host = host();
    let userdata = host.lua.create_any_userdata(5_u32).unwrap();
    host.lua.globals().set("handle", userdata).unwrap();
    host.run(
        "function f() local box = {inner = {handle}} coroutine.yield() end",
        "userdata.lua",
    );
    let error = refused(&host, &host.started("f", ()));
    let CaptureError::Userdata { path } = error else {
        panic!("unexpected {error}");
    };
    assert_eq!(path, "thread > level 1 > local `box` > [\"inner\"] > [1]");
}

#[test]
fn userdata_named_as_a_permanent_is_saved_by_name() {
    let mut host = host();
    let userdata = host.lua.create_any_userdata(5_u32).unwrap();
    host.permanents
        .insert("engine", Value::UserData(userdata.clone()))
        .unwrap();
    host.lua.globals().set("handle", userdata).unwrap();
    host.run(
        "function f() local h = handle coroutine.yield() end",
        "ud.lua",
    );
    let snapshot = host.capture(&host.started("f", ())).unwrap();
    let local = snapshot.frames()[0].locals.last().unwrap();
    assert_eq!(
        local.value,
        lua_resume::capture::Value::Permanent("engine".to_owned())
    );
}

#[test]
fn native_functions_that_are_not_permanents_are_refused() {
    let host = host();
    let native = host.lua.create_function(|_, ()| Ok(())).unwrap();
    host.lua.globals().set("native", native).unwrap();
    host.run(
        "function f() local callbacks = {native} coroutine.yield() end",
        "native.lua",
    );
    let error = refused(&host, &host.started("f", ()));
    let CaptureError::NativeFunction { path } = error else {
        panic!("unexpected {error}");
    };
    assert_eq!(path, "thread > level 1 > local `callbacks` > [1]");
}

#[test]
fn unnamed_stack_values_are_refused() {
    let host = host();
    raw(
        &host,
        "function f() for i = 1, 3 do coroutine.yield(i) end end",
        "loop.lua",
    );
    let error = refused(&host, &host.started("f", ()));
    let CaptureError::Unnamed { path, name } = error else {
        panic!("unexpected {error}");
    };
    assert_eq!(name, "(for state)");
    assert!(
        path.starts_with("thread > level 1 > local `(for state)`"),
        "{path}"
    );
}

#[test]
fn code_that_is_not_normalized_is_refused() {
    let host = host();
    raw(
        &host,
        "function f() local x = 1 coroutine.yield() end",
        "plain.lua",
    );
    let error = refused(&host, &host.started("f", ()));
    assert!(
        matches!(error, CaptureError::NotNormalized { ref path } if path == "thread > level 1"),
        "{error}"
    );
}

#[test]
fn a_nested_coroutine_outside_normalized_code_is_refused_with_its_path() {
    let host = host();
    raw(
        &host,
        "function child() local x = 1 coroutine.yield() end",
        "child.lua",
    );
    host.run(
        "function parent()
  local co = coroutine.create(child)
  coroutine.resume(co)
  coroutine.yield()
end
",
        "parent.lua",
    );
    let error = refused(&host, &host.started("parent", ()));
    let CaptureError::NotNormalized { path } = error else {
        panic!("unexpected {error}");
    };
    assert_eq!(path, "thread > level 1 > local `co` > level 1");
}

#[test]
fn a_native_frame_below_the_yield_is_refused() {
    let host = host();
    raw(
        &host,
        "function f() pcall(coroutine.yield) end",
        "pcall.lua",
    );
    let thread = host.lua.create_thread(host.global("f")).unwrap();
    thread.resume::<()>(()).unwrap();
    assert_eq!(thread.status(), ThreadStatus::Resumable);
    let error = refused(&host, &thread);
    assert!(
        matches!(error, CaptureError::NativeFrame { ref name, .. } if name == "pcall"),
        "{error}"
    );
}

#[test]
fn a_top_frame_that_is_not_a_known_yield_is_refused_and_a_named_native_is_accepted() {
    let host = host();
    host.run("function f() coroutine.yield() end", "top.lua");
    let thread = host.started("f", ());
    let lua = &host.lua;
    let yield_function: Value = lua
        .globals()
        .get::<mlua::Table>("coroutine")
        .unwrap()
        .get("yield")
        .unwrap();
    lua.globals().set("coroutine", Value::Nil).unwrap();
    let error = lua_resume::capture::capture(lua, &host.debug, &thread, &Permanents::new())
        .expect_err("the yield is not named");
    assert!(matches!(error, CaptureError::NotYielding { .. }), "{error}");
    let mut named = Permanents::new();
    named.insert("Sleep", yield_function).unwrap();
    named.insert("_G", Value::Table(lua.globals())).unwrap();
    let snapshot: Snapshot =
        lua_resume::capture::capture(lua, &host.debug, &thread, &named).unwrap();
    assert_eq!(
        snapshot.frames()[0].pending,
        lua_resume::capture::Callee::Native("Sleep".to_owned())
    );
}

#[test]
fn coroutines_that_are_not_suspended_are_refused() {
    let host = host();
    host.run("function f() coroutine.yield() end", "state.lua");
    let fresh = host.lua.create_thread(host.global("f")).unwrap();
    assert!(matches!(
        refused(&host, &fresh),
        CaptureError::NotStarted { .. }
    ));
    let thread = host.started("f", ());
    thread.resume::<()>(()).unwrap();
    assert!(matches!(
        refused(&host, &thread),
        CaptureError::NotSuspended {
            state: "finished",
            ..
        }
    ));
}

#[test]
fn different_functions_with_the_same_chunk_and_lines_are_refused() {
    let host = host();
    raw(&host, "other = function() end", "same.lua");
    raw(
        &host,
        "function f() local o = other coroutine.yield() end",
        "same.lua",
    );
    let error = refused(&host, &host.started("f", ()));
    assert!(matches!(error, CaptureError::Ambiguous { .. }), "{error}");
}

#[test]
fn snapshots_serialize_and_read_back_equal() {
    let host = host();
    host.run(
        "local shared = {}
shared.me = shared
function f(...)
  local data = {shared, shared, 1, 2.5, 'text', [true] = 'yes', nested = {'\\0\\255'}}
  local nan = string.unpack('<d', '\\1\\0\\0\\0\\0\\0\\248\\127')
  local fn = function() return shared end
  coroutine.yield()
end
",
        "serde.lua",
    );
    let snapshot = host.capture(&host.started("f", (1, Value::Nil))).unwrap();
    let text = serde_json::to_string(&snapshot).unwrap();
    let back: Snapshot = serde_json::from_str(&text).unwrap();
    assert_eq!(back, snapshot);
    assert_eq!(serde_json::to_string(&back).unwrap(), text);
}
