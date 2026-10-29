//! Which names of a function resolve to locals of the functions around it, by Lua 5.4 scoping.

use lua_resume::continuation::{FunctionKey, needed_upvalues};
use lua_resume::syntax::parse;

const NONE: [&str; 0] = [];

/// The upvalue names of the function that spans `line` to `end_line` of `source`.
fn needed(source: &str, line: u32, end_line: u32) -> Vec<String> {
    let tree = parse(source.as_bytes()).unwrap();
    needed_upvalues(&tree, FunctionKey { line, end_line }).unwrap()
}

#[test]
fn enclosing_locals_are_found_and_globals_are_not() {
    let source = "local a, b = 1, 2\nlocal function f()\n  return a + g + print\nend\n";
    assert_eq!(needed(source, 2, 4), ["a"]);
}

#[test]
fn a_name_is_listed_once_in_order_of_first_use() {
    let source = "local a, b = 1, 2\nlocal function f()\n  b = a + b\n  return a\nend\n";
    assert_eq!(needed(source, 2, 5), ["a", "b"]);
}

#[test]
fn a_local_of_the_function_hides_the_enclosing_one() {
    let source = "local a, b, c = 1, 2, 3\nlocal function f(b)\n  local a = 1\n  do local c = 2 end\n  return a + b + c\nend\n";
    assert_eq!(needed(source, 2, 6), ["c"]);
}

#[test]
fn a_nested_function_makes_its_free_names_upvalues_of_the_outer_one() {
    let source = "local a, b = 1, 2\nlocal function f()\n  local c = 3\n  return function()\n    return a + c\n  end\nend\n";
    assert_eq!(needed(source, 2, 7), ["a"]);
    assert_eq!(needed(source, 4, 6), ["a", "c"]);
}

#[test]
fn a_local_is_not_in_scope_in_its_own_initializer() {
    let source = "local x = function()\n  return x\nend\nlocal function y()\n  return y\nend\n";
    assert_eq!(needed(source, 1, 3), NONE);
    assert_eq!(needed(source, 4, 6), ["y"]);
}

#[test]
fn a_local_declared_after_the_function_is_a_global_in_it() {
    let source = "local function f()\n  return later\nend\nlocal later = 1\n";
    assert_eq!(needed(source, 1, 3), NONE);
}

#[test]
fn loop_variables_and_block_scopes_are_followed() {
    let source = "for i = 1, 2 do\n  local f = function() return i end\nend\nfor k, v in pairs({}) do\n  local g = function() return k, v end\nend\ndo local d = 1 end\nlocal h = function() return d, i end\n";
    assert_eq!(needed(source, 2, 2), ["i"]);
    assert_eq!(needed(source, 5, 5), ["k", "v"]);
    assert_eq!(needed(source, 8, 8), NONE);
}

#[test]
fn a_repeat_condition_sees_the_locals_of_the_body() {
    let source = "local function f()\n  repeat local done = true until done\n  return 1\nend\n";
    assert_eq!(needed(source, 1, 4), NONE);
}

#[test]
fn methods_and_the_environment_are_handled() {
    let source = "local obj = {}\nfunction obj:m()\n  return self, obj\nend\nlocal _ENV = {}\nlocal function g()\n  return x\nend\n";
    assert_eq!(needed(source, 2, 4), ["obj"]);
    assert_eq!(needed(source, 6, 8), NONE);
}

#[test]
fn a_function_statement_on_an_enclosing_local_uses_it() {
    let source = "local t = {}\nlocal function f()\n  function t.x() end\nend\n";
    assert_eq!(needed(source, 2, 4), ["t"]);
}
