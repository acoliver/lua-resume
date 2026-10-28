//! Reproductions from independent semantic review.

mod support;

use support::normalize_source;

use lua_resume::{rewrite::check_normal_form, syntax::print};
use mlua::Lua;

fn run(source: &str) -> String {
    Lua::new().load(source).eval().unwrap()
}

fn equivalent(source: &str) {
    let tree = normalize_source(source.as_bytes()).unwrap();
    check_normal_form(&tree).unwrap();
    let rewritten = print(&tree);
    assert_eq!(run(source), run(&rewritten), "{rewritten}");
}

#[test]
fn assignment_conflicts_snapshot_before_calls() {
    for assignment in ["t[i],i=f()", "t[i],i=f()+1,4", "t.x,t=f()"] {
        let source = format!(
            "local t,i={{}},1; local old=t; local function f() i=2; t={{}}; return 3,4 end; {assignment}; return tostring(old[1])..':'..tostring(old[2])..':'..tostring(old.x)"
        );
        equivalent(&source);
    }
}

#[test]
fn method_lookup_precedes_every_argument() {
    equivalent(
        "local arg={x=1}; local obj=setmetatable({}, {__index=function() arg.x=2; return function(_,x) return x end end}); local function f() return 0 end; return tostring(obj:m(arg.x,f()))",
    );
}

#[test]
fn implicit_environment_is_an_upvalue() {
    equivalent(
        "local function change() _ENV={x='new'}; return 'x' end; _ENV={x='old'}; return _ENV[change()]",
    );
    equivalent(
        "local function change() _ENV={x='new'}; return 'x' end; _ENV={x='old'}; local function get() return _ENV[change()] end; return get()",
    );
}

#[test]
fn duplicate_iterator_variables_keep_the_first_control_result() {
    equivalent(
        "local n=0; local function it() n=n+1; if n<=2 then return n,nil end end; local s=''; for x,x in it do s=s..tostring(x) end; return s",
    );
}

#[test]
fn keyed_store_errors_precede_later_calls() {
    for constructor in ["{[nil]=1,f()+1}", "{[nil]=1,f(),f()}", "{[0/0]=1,f()}"] {
        let source = format!(
            "local n=0; local function f() n=n+1; return 1 end; pcall(function() local t={constructor} end); return tostring(n)"
        );
        equivalent(&source);
    }
}

#[test]
fn local_budget_failure_is_explicit() {
    let names = (1..=199)
        .map(|i| format!("v{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let source = format!("local {names}; return (function() return 'ok' end)() .. ''");
    assert_eq!(run(&source), "ok");
    let error = normalize_source(source.as_bytes()).unwrap_err();
    assert!(error.message.contains("local"), "{error}");
}

#[test]
fn assignment_tables_follow_indexing_read_points() {
    for assignment in ["_ENV.x=f()", "_ENV[f()]=1", "(_ENV).x=f()", "(_ENV)[f()]=1"] {
        let source = format!(
            "local old={{}}; local function f() _ENV={{}}; return 'x' end; _ENV=old; {assignment}; return old.x and 'old' or 'new'"
        );
        equivalent(&source);
    }
}

#[test]
fn constructor_local_keys_are_read_after_values() {
    equivalent(
        "local k=1; local function f() k=2; return 3 end; local t={[k]=f()}; return tostring(t[1])..':'..tostring(t[2])",
    );
    equivalent(
        "return tostring(pcall(function() local k=1; local function f() k=nil; return 3 end; local t={[k]=f()} end))",
    );
    equivalent(
        "local k='before'; local a=setmetatable({}, {__index=function() k='after'; return 3 end}); local function f() return 4 end; local t={[k]=a.x,f()}; return tostring(t.before)..':'..tostring(t.after)",
    );
}

#[test]
fn parenthesized_upvalue_tables_are_materialized_before_the_key() {
    equivalent(
        "local function change() _ENV={x='new'}; return 'x' end; _ENV={x='old'}; return (_ENV)[change()]",
    );
}

#[test]
fn upvalue_table_timing_survives_constant_table_saturation() {
    let constants = (1..=300)
        .map(|i| format!("'constant{i}'"))
        .collect::<Vec<_>>()
        .join(",");
    let source = format!(
        "local constants={{{constants}}}; local old={{}}; local function f() _ENV={{}}; return 1 end; _ENV=old; _ENV.x=f(); return old.x and 'old' or 'new'"
    );
    equivalent(&source);
}
