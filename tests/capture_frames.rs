//! Capturing normalized fixtures: frame identity, sites, locals and shared data.

mod capture_support;

use capture_support::host;
use lua_resume::capture::{Callee, Snapshot, ThreadData, Value};
use mlua::{Thread, ThreadStatus, Value as Lua};

const WORKER: &str = "function worker(n)
  local total = 0
  local i = 0
  while i < n do
    i = i + 1
    local got = coroutine.yield(i)
    total = total + got
  end
  return total
end
";

fn local<'a>(snapshot: &'a Snapshot, frame: usize, name: &str) -> &'a Value {
    let found = snapshot.frames()[frame]
        .locals
        .iter()
        .find(|local| local.name == name)
        .unwrap_or_else(|| panic!("no local {name}"));
    &found.value
}

fn names(snapshot: &Snapshot, frame: usize) -> Vec<&str> {
    snapshot.frames()[frame]
        .locals
        .iter()
        .map(|local| local.name.as_str())
        .collect()
}

#[test]
fn every_yield_of_a_worker_captures_identity_site_and_locals() {
    let host = host();
    host.run(WORKER, "worker.lua");
    let thread = host.started("worker", 3);
    let mut total = 0;
    for turn in 1..=3 {
        let snapshot = host.capture(&thread).unwrap();
        assert_eq!(snapshot.frames().len(), 1);
        let frame = &snapshot.frames()[0];
        assert_eq!(frame.function.chunk, "=worker.lua");
        assert_eq!((frame.function.line, frame.function.end_line), (1, 10));
        assert_eq!(frame.site, 1);
        assert_eq!(frame.pending, Callee::Native("coroutine.yield".to_owned()));
        assert_eq!(names(&snapshot, 0), ["n", "__lrs", "total", "i"]);
        assert_eq!(local(&snapshot, 0, "n"), &Value::Integer(3));
        assert_eq!(local(&snapshot, 0, "__lrs"), &Value::Integer(1));
        assert_eq!(local(&snapshot, 0, "total"), &Value::Integer(total));
        assert_eq!(local(&snapshot, 0, "i"), &Value::Integer(turn));
        total += 10 * turn;
        thread.resume::<mlua::MultiValue>(10 * turn).unwrap();
    }
    assert_eq!(thread.status(), ThreadStatus::Finished);
}

const CALLS: &str = "local shared = {name = 'shared'}
shared.self = shared
local counter = 0
local function bump() counter = counter + 1 return counter end
local function peek() return counter end
function inner(a, ...)
  local t = {shared, shared, bump, peek}
  coroutine.yield(t)
  return a
end
function outer(...)
  local x = 1.5
  local r = inner('a', ...)
  return r
end
";

fn start_calls(host: &capture_support::Host) -> Thread {
    host.run(CALLS, "calls.lua");
    host.started("outer", (7, Lua::Nil, "z"))
}

#[test]
fn nested_calls_capture_outermost_first_with_varargs_and_pending_calls() {
    let host = host();
    let snapshot = host.capture(&start_calls(&host)).unwrap();
    let frames = snapshot.frames();
    assert_eq!(frames.len(), 2);
    let (outer, inner) = (&frames[0], &frames[1]);
    assert_eq!((outer.function.line, outer.function.end_line), (11, 15));
    assert_eq!((inner.function.line, inner.function.end_line), (6, 10));
    assert_eq!(outer.pending, Callee::Lua(inner.function.clone()));
    assert_eq!(inner.pending, Callee::Native("coroutine.yield".to_owned()));
    assert_eq!(
        outer.varargs,
        [Value::Integer(7), Value::Nil, Value::String(b"z".to_vec())]
    );
    assert_eq!(inner.varargs, outer.varargs);
    assert_eq!(local(&snapshot, 0, "x"), &Value::float(1.5));
    assert_eq!(local(&snapshot, 1, "a"), &Value::String(b"a".to_vec()));
    assert_eq!((outer.site, inner.site), (1, 1));
}

#[test]
fn tables_keep_identity_cycles_and_closures_share_upvalue_cells() {
    let host = host();
    let snapshot = host.capture(&start_calls(&host)).unwrap();
    let Value::Table(list) = *local(&snapshot, 1, "t") else {
        panic!("t is a table");
    };
    let entries = &snapshot.tables[list].entries;
    let Value::Table(shared) = entries[0].1 else {
        panic!("first entry is a table");
    };
    assert_eq!(entries[1].1, Value::Table(shared));
    let named = &snapshot.tables[shared].entries;
    assert!(named.contains(&(Value::String(b"self".to_vec()), Value::Table(shared))));
    let (Value::Closure(bump), Value::Closure(peek)) = (&entries[2].1, &entries[3].1) else {
        panic!("closures expected");
    };
    let cell = |closure: usize| {
        snapshot.closures[closure]
            .upvalues
            .iter()
            .find(|upvalue| upvalue.name == "counter")
            .unwrap()
            .cell
    };
    assert_eq!(cell(*bump), cell(*peek));
    assert_eq!(snapshot.cells[cell(*bump)].value, Value::Integer(0));
    assert_eq!(snapshot.closures[*bump].function.line, 4);
}

#[test]
fn a_running_closure_is_captured_with_its_upvalues_and_environment() {
    let host = host();
    let snapshot = host.capture(&start_calls(&host)).unwrap();
    let closure = &snapshot.closures[snapshot.frames()[1].closure];
    assert_eq!(closure.function, snapshot.frames()[1].function);
    let env = closure
        .upvalues
        .iter()
        .find(|up| up.name == "_ENV")
        .unwrap();
    assert_eq!(
        snapshot.cells[env.cell].value,
        Value::Permanent("_G".to_owned())
    );
}

#[test]
fn numbers_and_strings_keep_their_exact_representation() {
    let host = host();
    host.run(
        "function numbers()
  local int, float, whole = 1, 1.0, 2^53
  local nan = string.unpack('<d', '\\1\\0\\0\\0\\0\\0\\248\\127')
  local inf, negzero = 1 / 0, -(0.0)
  local bytes = '\\0\\255a'
  coroutine.yield()
end
",
        "numbers.lua",
    );
    let snapshot = host.capture(&host.started("numbers", ())).unwrap();
    assert_eq!(local(&snapshot, 0, "int"), &Value::Integer(1));
    assert_eq!(local(&snapshot, 0, "float"), &Value::float(1.0));
    assert_eq!(
        local(&snapshot, 0, "whole"),
        &Value::float(9_007_199_254_740_992.0)
    );
    assert_eq!(
        local(&snapshot, 0, "nan"),
        &Value::Float(0x7ff8_0000_0000_0001)
    );
    assert_eq!(local(&snapshot, 0, "inf"), &Value::float(f64::INFINITY));
    assert_eq!(local(&snapshot, 0, "negzero"), &Value::float(-0.0));
    assert_eq!(
        local(&snapshot, 0, "bytes"),
        &Value::String(vec![0, 255, b'a'])
    );
}

#[test]
fn table_entries_are_ordered_by_key_whatever_the_insertion_order() {
    let build = |order: &str| {
        let host = host();
        host.run(
            &format!("function f() local t = {{}} {order} coroutine.yield() end"),
            "order.lua",
        );
        host.capture(&host.started("f", ())).unwrap()
    };
    let first = build("t.b = 1 t.a = 2 t[3] = true t[1] = 'x' t[2.5] = false");
    let second = build("t[2.5] = false t[1] = 'x' t[3] = true t.a = 2 t.b = 1");
    assert_eq!(first, second);
    let Value::Table(id) = *local(&first, 0, "t") else {
        panic!("t is a table");
    };
    let keys: Vec<&Value> = first.tables[id]
        .entries
        .iter()
        .map(|(key, _)| key)
        .collect();
    assert_eq!(
        keys,
        [
            &Value::Integer(1),
            &Value::Integer(3),
            &Value::float(2.5),
            &Value::String(b"a".to_vec()),
            &Value::String(b"b".to_vec()),
        ]
    );
}

#[test]
fn metatables_are_recorded_and_native_functions_are_saved_by_name() {
    let host = host();
    host.run(
        "function f()
  local meta = {__index = string}
  local t = setmetatable({}, meta)
  local show = tostring
  coroutine.yield()
end
",
        "meta.lua",
    );
    let snapshot = host.capture(&host.started("f", ())).unwrap();
    let Value::Table(id) = *local(&snapshot, 0, "t") else {
        panic!("t is a table");
    };
    let meta = snapshot.tables[id].metatable.unwrap();
    assert_eq!(
        snapshot.tables[meta].entries,
        [(
            Value::String(b"__index".to_vec()),
            Value::Permanent("string".to_owned())
        )]
    );
    assert_eq!(
        local(&snapshot, 0, "show"),
        &Value::Permanent("tostring".to_owned())
    );
}

#[test]
fn coroutines_on_the_stack_are_captured_once_and_dead_ones_are_marked() {
    let host = host();
    host.run(
        "function child() local mark = 5 coroutine.yield() end
function parent()
  local a = coroutine.create(child)
  local b = a
  local done = coroutine.create(function() end)
  coroutine.resume(a)
  coroutine.resume(done)
  coroutine.yield()
end
",
        "threads.lua",
    );
    let snapshot = host.capture(&host.started("parent", ())).unwrap();
    let (Value::Thread(a), Value::Thread(b), Value::Thread(done)) = (
        local(&snapshot, 0, "a"),
        local(&snapshot, 0, "b"),
        local(&snapshot, 0, "done"),
    ) else {
        panic!("threads expected");
    };
    assert_eq!(a, b);
    assert_eq!(snapshot.threads[*done], ThreadData::Dead);
    let ThreadData::Suspended { frames } = &snapshot.threads[*a] else {
        panic!("child is suspended");
    };
    assert_eq!(frames[0].function.line, 1);
    assert_eq!(frames[0].locals.last().unwrap().value, Value::Integer(5));
}

#[test]
fn a_long_chain_of_tables_does_not_overflow_the_stack() {
    let host = host();
    host.run(
        "function f()
  local head = nil
  for i = 1, 100000 do head = {next = head, value = i} end
  coroutine.yield()
end
",
        "chain.lua",
    );
    let snapshot = host.capture(&host.started("f", ())).unwrap();
    assert_eq!(snapshot.tables.len(), 100_000);
}
