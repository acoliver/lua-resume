//! Continuations resumed at each yield of a normalized worker finish like the original.

mod support;

use lua_resume::continuation::{ContinuationError, FunctionKey, continuation_source};
use lua_resume::syntax::{ast::Block, print};
use mlua::{Function, Lua, Table, Thread, Value};
use support::normalize_source;

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
const KEY: FunctionKey = FunctionKey {
    line: 1,
    end_line: 10,
};
const LOCALS: [&str; 5] = ["n", "__lrs", "__lr1", "total", "i"];

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

fn normalized() -> Block {
    normalize_source(WORKER.as_bytes()).unwrap()
}

/// Yields and the final return of `worker(3)`, resumed with 10, 20, 30.
fn uninterrupted() -> Vec<i64> {
    let lua = Lua::new();
    lua.load(print(&normalized())).exec().unwrap();
    let worker: Function = lua.globals().get("worker").unwrap();
    let thread = lua.create_thread(worker).unwrap();
    let mut seen = vec![thread.resume::<i64>(3).unwrap()];
    seen.push(thread.resume::<i64>(10).unwrap());
    seen.push(thread.resume::<i64>(20).unwrap());
    seen.push(thread.resume::<i64>(30).unwrap());
    seen
}

/// Continues `worker` from a yield with the saved locals, then resumes with `inputs`.
fn resumed(saved: [i64; 3], site: u32, inputs: &[i64]) -> Vec<i64> {
    let lua = Lua::new();
    lua.load(print(&normalized())).exec().unwrap();
    let source = continuation_source(&normalized(), KEY, site, &names(&LOCALS), &[], &[]).unwrap();
    let continuation: Function = lua.load(&source).eval().unwrap();
    let values = lua.create_table().unwrap();
    for (index, value) in [
        Value::Integer(3),
        Value::Integer(i64::from(site)),
        Value::Nil,
    ]
    .into_iter()
    .chain(saved[1..].iter().map(|v| Value::Integer(*v)))
    .enumerate()
    {
        values.set(index + 1, value).unwrap();
    }
    let next: Function = lua
        .load("return function() return coroutine.yield() end")
        .eval()
        .unwrap();
    let unpack: Table = lua.globals().get("table").unwrap();
    let start: Function = lua
        .load("local c, v, n, u = ... return function() return c(v, n, u) end")
        .call((
            continuation,
            values,
            next,
            unpack.get::<Function>("unpack").unwrap(),
        ))
        .unwrap();
    let thread: Thread = lua.create_thread(start).unwrap();
    thread.resume::<()>(()).unwrap();
    inputs
        .iter()
        .map(|input| thread.resume::<i64>(*input).unwrap())
        .collect()
}

#[test]
fn resuming_at_every_yield_matches_the_uninterrupted_run() {
    let expected = uninterrupted();
    assert_eq!(expected, vec![1, 2, 3, 60]);
    // Saved (n, total, i) after each yield; the site is the same call statement each time.
    let site = 1;
    assert_eq!(resumed([3, 0, 1], site, &[10, 20, 30]), expected[1..]);
    assert_eq!(resumed([3, 10, 2], site, &[20, 30]), expected[2..]);
    assert_eq!(resumed([3, 30, 3], site, &[30]), expected[3..]);
}

#[test]
fn an_unknown_function_or_site_is_an_error() {
    let tree = normalized();
    let other = FunctionKey {
        line: 4,
        end_line: 5,
    };
    assert_eq!(
        continuation_source(&tree, other, 1, &[], &[], &[]),
        Err(ContinuationError::NoFunction(4, 5))
    );
    assert_eq!(
        continuation_source(&tree, KEY, 9, &[], &[], &[]),
        Err(ContinuationError::NoSite(9))
    );
}
