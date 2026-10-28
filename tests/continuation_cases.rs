//! Continuations for return sites, varargs, loops, labels, field stores and `pcall` frames
//! finish like the original when resumed from every yield in a fresh Lua state.

mod harness;
mod support;

use harness::{Case, check};
use lua_resume::continuation::{
    ContinuationError, FunctionKey, PROTECTED_NEXT, continuation_source,
};
use lua_resume::syntax::{parse, print};
use mlua::{Function, Lua, Thread};
use support::normalize_source;

fn key(line: u32, end_line: u32) -> FunctionKey {
    FunctionKey { line, end_line }
}

#[test]
fn return_sites_and_varargs_continue_with_the_saved_varargs() {
    let case = Case {
        source: "function f(a, ...)
  local n = select('#', ...)
  local x = coroutine.yield(a)
  trace = (trace or '') .. n .. ':' .. tostring(x) .. ':' .. select('#', ...) .. ';'
  return coroutine.yield(x, ...)
end
",
        function: "f",
        key: key(1, 6),
        args: "{1, 'x', nil, 3, n = 4}",
        inputs: &["{7}", "{8, 9, 10}"],
        globals: &["trace"],
    };
    assert_eq!(check(&case), 2);
}

#[test]
fn a_return_with_a_call_chain_and_several_results_continues() {
    let case = Case {
        source: "function g(a, ...)
  local t = {...}
  return add(a, coroutine.yield(#t)), 'end'
end
function add(a, b) return a + b, a * b end
",
        function: "g",
        key: key(1, 4),
        args: "{5, 1, 2}",
        inputs: &["{30}"],
        globals: &[],
    };
    assert_eq!(check(&case), 1);
}

#[test]
fn numeric_for_loops_resume_with_integer_and_float_control_values() {
    let case = Case {
        source: "function loop(n)
  local sum = 0
  for i = 1, n do
    local got = coroutine.yield(i)
    sum = sum + got * i
    i = i + 100
    trace = (trace or '') .. i .. ','
  end
  for x = 1.5, 0, -0.5 do
    sum = sum + coroutine.yield(x)
  end
  for k = math.maxinteger - 1, math.maxinteger do
    sum = sum + coroutine.yield(k) % 7
  end
  return sum
end
",
        function: "loop",
        key: key(1, 16),
        args: "{3}",
        inputs: &[
            "{1}", "{2}", "{3}", "{10}", "{20}", "{30}", "{40}", "{1}", "{2}",
        ],
        globals: &["trace"],
    };
    assert_eq!(check(&case), 9);
}

#[test]
fn generic_for_loops_continue_over_ipairs_and_next() {
    let case = Case {
        source: "function walk(t)
  local out = 0
  for i, v in ipairs(t) do out = out + coroutine.yield(i, v) end
  for k, v in next, t do out = out + coroutine.yield(k, v) end
  for k, v in pairs(t) do
    out = out * 2 + coroutine.yield(k, v)
  end
  return out
end
",
        function: "walk",
        key: key(1, 9),
        args: "{{10, 20, 30}}",
        inputs: &[
            "{1}", "{2}", "{3}", "{1}", "{2}", "{3}", "{1}", "{2}", "{3}",
        ],
        globals: &[],
    };
    assert_eq!(check(&case), 9);
}

#[test]
fn repeat_until_sees_body_locals_after_a_restore() {
    let case = Case {
        source: "function rep(n)
  local i = 0
  repeat
    i = i + 1
    local done = coroutine.yield(i) >= n
    trace = (trace or '') .. tostring(done) .. ','
  until done or i > 5
  return i
end
",
        function: "rep",
        key: key(1, 9),
        args: "{3}",
        inputs: &["{1}", "{2}", "{3}"],
        globals: &["trace"],
    };
    assert_eq!(check(&case), 3);
}

#[test]
fn continue_labels_after_the_call_survive_a_restore() {
    let skipping = Case {
        source: "function cont(n)
  local s = 0
  for i = 1, n do
    if i % 2 == 0 then goto continue end
    s = s + coroutine.yield(i)
    ::continue::
  end
  return s
end
",
        function: "cont",
        key: key(1, 9),
        args: "{5}",
        inputs: &["{1}", "{2}", "{3}"],
        globals: &[],
    };
    assert_eq!(check(&skipping), 3);
}

#[test]
fn field_assignments_from_a_call_continue() {
    let case = Case {
        source: "function fields()
  local t = {}
  t.x = coroutine.yield('x')
  t[2], t.y = coroutine.yield('two')
  local k = 'dyn'
  t[k] = coroutine.yield('dyn')
  return t.x, t[2], t.y, t.dyn
end
",
        function: "fields",
        key: key(1, 8),
        args: "{}",
        inputs: &["{1}", "{2, 3}", "{4}"],
        globals: &[],
    };
    assert_eq!(check(&case), 3);
}

const PROTECTED: &str = "function inner(n)
  local got = coroutine.yield(n)
  if got == 'boom' then error('bad', 0) end
  return got
end
function outer()
  local ok, err = pcall(inner, 1)
  error('after ' .. tostring(ok) .. ' ' .. tostring(err), 0)
end
";

/// Resumes the saved frames of `outer` (waiting in `pcall`) and `inner` (waiting in the yield)
/// and returns what the second resume yields or raises.
fn protected_run(input: &str) -> String {
    let tree = normalize_source(PROTECTED.as_bytes()).unwrap();
    let lua = Lua::new();
    lua.load(print(&tree)).exec().unwrap();
    let inner_source =
        continuation_source(&tree, key(1, 5), 1, &["n".into(), "__lrs".into()], &[], &[]).unwrap();
    let outer_source =
        continuation_source(&tree, key(6, 9), 1, &["__lrs".into()], &[], &[]).unwrap();
    let wrap: Function = lua.load(PROTECTED_NEXT).eval().unwrap();
    let driver = lua
        .load(
            "local inner, outer, wrap, input = ...
            local unpack = table.unpack
            local yield = function(...) return coroutine.yield(...) end
            local frame = function()
              return inner({1, 1, n = 2}, yield, unpack)
            end
            local co = coroutine.create(function()
              return outer({1, n = 1}, wrap(frame), unpack)
            end)
            assert(coroutine.resume(co))
            local ok, message = coroutine.resume(co, input)
            return tostring(ok) .. ',' .. tostring(message)",
        )
        .into_function()
        .unwrap();
    let inner: Function = lua.load(&inner_source).eval().unwrap();
    let outer: Function = lua.load(&outer_source).eval().unwrap();
    driver.call((inner, outer, wrap, input)).unwrap()
}

fn protected_original(input: &str) -> String {
    let tree = normalize_source(PROTECTED.as_bytes()).unwrap();
    let lua = Lua::new();
    lua.load(print(&tree)).exec().unwrap();
    let outer: Function = lua.globals().get("outer").unwrap();
    let thread: Thread = lua.create_thread(outer).unwrap();
    thread.resume::<mlua::MultiValue>(()).unwrap();
    let error = thread.resume::<mlua::MultiValue>(input).unwrap_err();
    format!(
        "false,{}",
        error.to_string().trim_start_matches("runtime error: ")
    )
}

#[test]
fn errors_after_a_restore_inside_pcall_are_caught_like_the_original() {
    for (input, expected) in [("boom", "after false bad"), ("fine", "after true fine")] {
        let resumed = protected_run(input);
        assert_eq!(resumed, format!("false,{expected}"));
        assert!(protected_original(input).starts_with(&resumed), "{input}");
    }
}

#[test]
fn unsupported_shapes_are_refused_with_the_construct_named() {
    let source = "function f()\n  ::top::\n  coroutine.yield(1)\n  goto top\nend\n";
    let tree = normalize_source(source.as_bytes()).unwrap();
    assert_eq!(
        continuation_source(&tree, key(1, 5), 1, &[], &[], &[]),
        Err(ContinuationError::Unsupported(
            "a goto back to a label before the call"
        ))
    );
    let source =
        "function f()\n  for i = 1, 2 do\n    __lrs = 1\n    coroutine.yield(i)\n  end\nend\n";
    let tree = parse(source.as_bytes()).unwrap();
    assert_eq!(
        continuation_source(&tree, key(1, 6), 1, &[], &[], &[]),
        Err(ContinuationError::Unsupported(
            "a numeric for loop that was not normalized"
        ))
    );
}
