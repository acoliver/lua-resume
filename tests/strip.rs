//! Statement-level `assert` and `dbg` removal.

use lua_resume::strip::strip_debug_calls;
use lua_resume::syntax::{parse, print};

fn stripped(source: &str) -> (String, usize) {
    let mut tree = parse(source.as_bytes()).expect("the source parses");
    let removed = strip_debug_calls(&mut tree);
    (print(&tree), removed)
}

fn same_after_strip(source: &str) {
    let (text, removed) = stripped(source);
    assert_eq!(removed, 0, "{text}");
    assert_eq!(text, print(&parse(source.as_bytes()).unwrap()));
}

#[test]
fn call_statements_of_the_globals_are_removed() {
    let (text, removed) = stripped("assert(x, 'm')\ndbg('v', f())\nprint(1)\n");
    assert_eq!(removed, 2);
    assert_eq!(text, print(&parse(b"\n\nprint(1)").unwrap()));
}

#[test]
fn removal_reaches_nested_blocks_and_function_bodies() {
    let source = "local f = function() assert(1) end\nfor i = 1, 2 do dbg(i) end\nif x then assert(x) else dbg() end\nrepeat assert(1) until true\n";
    let (text, removed) = stripped(source);
    assert_eq!(removed, 5);
    assert!(!text.contains("assert") && !text.contains("dbg"), "{text}");
}

#[test]
fn a_local_named_assert_keeps_its_calls() {
    same_after_strip("local assert = print\nassert(1)\n");
}

#[test]
fn parameters_and_loop_variables_shadow() {
    same_after_strip("local function f(dbg) dbg(1) end");
    same_after_strip("for assert in pairs(t) do assert(1) end");
    same_after_strip("local function assert() end\nassert(1)");
}

#[test]
fn shadowing_ends_with_its_scope() {
    let (text, removed) = stripped("do local assert = print\nassert(1)\nend\nassert(2)\n");
    assert_eq!(removed, 1);
    assert!(text.contains("print"), "{text}");
    assert!(!text.contains("assert(2)"), "{text}");
}

#[test]
fn a_local_is_visible_only_after_its_statement() {
    let (_, removed) = stripped("local assert = assert(1) or print\n");
    assert_eq!(removed, 0);
    let (_, removed) = stripped("local x = function() assert(1) end\nlocal assert = 1\n");
    assert_eq!(removed, 1);
}

#[test]
fn repeat_condition_sees_body_locals_only_inside_the_loop() {
    let (_, removed) = stripped("repeat local assert = 1 until assert\nassert(1)\n");
    assert_eq!(removed, 1);
}

#[test]
fn calls_inside_expressions_stay() {
    same_after_strip("local ok = assert(x)\nreturn assert(y)\n");
    same_after_strip("f(assert(x))\nlocal t = { assert(x) }\n");
    same_after_strip("x = dbg(1) or 2\n");
}

#[test]
fn fields_methods_and_parenthesized_callees_stay() {
    same_after_strip("t.assert(1)\nobj:assert(1)\nobj:dbg()\n");
    same_after_strip("(assert)(1)\nt.dbg.assert(1)\n");
}

#[test]
fn a_local_environment_stops_removal() {
    same_after_strip("local _ENV = {}\nassert(1)\n");
}

#[test]
fn method_functions_see_self_as_a_local() {
    let (_, removed) = stripped("function t:m() assert(1) end\n");
    assert_eq!(removed, 1);
}

#[test]
fn other_statements_keep_their_line_numbers() {
    let mut tree = parse(b"assert(1)\nlocal x = 1\n\nprint(x)\n").unwrap();
    strip_debug_calls(&mut tree);
    let lines: Vec<u32> = tree.stats.iter().map(|stat| stat.line).collect();
    assert_eq!(lines, [2, 4]);
}
